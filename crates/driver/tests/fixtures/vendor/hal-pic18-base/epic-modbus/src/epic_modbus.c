/* Modbus RTU slave core: CRC-16, T3.5 silence-delimited framing,
 * function-code dispatch, optional RS-485 direction control.
 * Single-instance module: state lives in file-scope statics, the same
 * model lib/serial/epic-tick use; no #if needed here. */

#include "epic_modbus.h"
#include "epic_hal.h"
#include "epic_serial.h"
#include "epic_tick.h"

#include <stdbool.h>

/* Modbus function codes (the "core" set this module implements) */
#define MB_FC_READ_COILS            0x01u
#define MB_FC_READ_DISCRETE_INPUTS  0x02u
#define MB_FC_READ_HOLDING_REGS     0x03u
#define MB_FC_READ_INPUT_REGS       0x04u
#define MB_FC_WRITE_SINGLE_COIL     0x05u
#define MB_FC_WRITE_SINGLE_REG      0x06u
#define MB_FC_WRITE_MULTIPLE_COILS  0x0Fu
#define MB_FC_WRITE_MULTIPLE_REGS   0x10u

#define MB_EXC_ILLEGAL_FUNCTION      0x01u
#define MB_EXC_ILLEGAL_DATA_ADDRESS  0x02u
#define MB_EXC_ILLEGAL_DATA_VALUE    0x03u

/* module state */
static uint8_t  s_frame[EPIC_MODBUS_MAX_ADU];
static uint16_t s_frame_len;
static uint32_t s_last_rx_tick;

static uint8_t                        s_slave_addr;
static const epic_modbus_slave_map_t *s_map;
static uint32_t                       s_t3_5_ms;

static uint8_t s_dir_port;
static uint8_t s_dir_pin;
static bool    s_dir_configured;

/**
 * @brief  CRC-16 (Modbus/ANSI, poly 0xA001, init 0xFFFF), bit-loop.
 * @param buf the bytes to checksum.
 * @param len number of bytes in `buf`.
 * @return the CRC-16 value.
 */
static uint16_t modbus_crc16(const uint8_t *buf, uint16_t len)
{
    uint16_t crc = 0xFFFFu;
    for (uint16_t i = 0; i < len; i++)
    {
        crc = (uint16_t)(crc ^ buf[i]);
        for (uint8_t bit = 0; bit < 8u; bit++)
        {
            if (crc & 0x0001u)
            {
                crc = (uint16_t)((crc >> 1) ^ 0xA001u);
            }
            else
            {
                crc = (uint16_t)(crc >> 1);
            }
        }
    }
    return crc;
}

/**
 * @brief  T3.5 inter-frame silence timeout in milliseconds (see README
 *         for the >19200 baud caveat).
 * @param baud the RTU baud rate.
 * @return the silence timeout in ms, rounded up so it never falls short
 *         of the true requirement.
 */
static uint32_t compute_t3_5_ms(uint32_t baud)
{
    if (baud > 19200u)
    {
        return 2u; /* spec fixes T3.5 = 1.75 ms above 19200 baud; epic-tick's
                       1 ms resolution rounds that up to 2 ticks. */
    }
    /* ceil(3.5 chars * 11 bits/char * 1000 ms/s / baud); round up so the
     * timeout never falls short of the true silence requirement. */
    return (uint32_t)((38500ul + baud - 1ul) / baud);
}

/* bit-packed coil/discrete-input helpers */
/**
 * @brief  Read one bit from a bit-packed table.
 * @param arr the bit-packed array.
 * @param idx the bit index (LSB of arr[0] is index 0).
 * @return the bit's value.
 */
static bool bit_get(const uint8_t *arr, uint16_t idx)
{
    return (bool)((arr[idx >> 3] >> (idx & 7u)) & 1u);
}

/**
 * @brief  Write one bit into a bit-packed table.
 * @param arr the bit-packed array.
 * @param idx the bit index (LSB of arr[0] is index 0).
 * @param v   the value to set.
 */
static void bit_set(uint8_t *arr, uint16_t idx, bool v)
{
    if (v)
    {
        arr[idx >> 3] = (uint8_t)(arr[idx >> 3] | (uint8_t)(1u << (idx & 7u)));
    }
    else
    {
        arr[idx >> 3] = (uint8_t)(arr[idx >> 3] & (uint8_t)~(1u << (idx & 7u)));
    }
}

/**
 * @brief  Read a big-endian uint16_t from a byte pair.
 * @param p pointer to the two bytes (high byte first).
 * @return the value.
 */
static uint16_t be16(const uint8_t *p)
{
    return (uint16_t)(((uint16_t)p[0] << 8) | p[1]);
}

/**
 * @brief  Write a uint16_t as a big-endian byte pair.
 * @param p the destination byte pair (high byte written first).
 * @param v the value to write.
 */
static void put_be16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)v;
}

/* Response builders. Return the PDU length (addr+fc+payload, before
 * CRC) written into resp[], or 0 for "drop, no response" (only used for a
 * frame whose length doesn't match its own function code, which passing
 * CRC makes vanishingly unlikely, this is defensive, not spec-driven). */

/**
 * @brief  Build an exception response (addr, fc|0x80, exception code).
 * @param resp     the response buffer to fill.
 * @param fc       the function code being answered.
 * @param exc_code the Modbus exception code.
 * @return the PDU length written (always 3).
 */
static uint16_t build_exception(uint8_t *resp, uint8_t fc, uint8_t exc_code)
{
    resp[0] = s_slave_addr;
    resp[1] = (uint8_t)(fc | 0x80u);
    resp[2] = exc_code;
    return 3u;
}

/**
 * @brief  Handle an FC01/FC02 read-bits request: validate, pack the
 *         requested bits, and build the response.
 * @param fc        the function code (echoed into the response).
 * @param table     the bit-packed table to read, or NULL if absent.
 * @param table_len the table's bit count.
 * @param resp      the response buffer to fill.
 * @return the PDU length written, or 0 to drop the request.
 */
static uint16_t handle_read_bits(uint8_t fc, const uint8_t *table, uint16_t table_len,
                                  uint8_t *resp)
{
    if (s_frame_len != 8u)
    { /* addr+fc+4 payload+crc16 */
        return 0u;
    }
    uint16_t start = be16(&s_frame[2]);
    uint16_t qty   = be16(&s_frame[4]);

    if (qty == 0u || qty > 2000u)
    {
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_VALUE);
    }
    if (table == NULL || (uint32_t)start + qty > table_len)
    {
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }
    uint16_t byte_count = (uint16_t)((qty + 7u) / 8u);
    if ((uint32_t)byte_count + 5u > EPIC_MODBUS_MAX_ADU)
    { /* +5 = addr+fc+bytecount+crc16 */
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_VALUE);
    }

    resp[0] = s_slave_addr;
    resp[1] = fc;
    resp[2] = (uint8_t)byte_count;
    for (uint16_t i = 0; i < byte_count; i++)
    {
        resp[3 + i] = 0u;
    }
    for (uint16_t i = 0; i < qty; i++)
    {
        if (bit_get(table, (uint16_t)(start + i)))
        {
            resp[3 + (i >> 3)] = (uint8_t)(resp[3 + (i >> 3)] | (uint8_t)(1u << (i & 7u)));
        }
    }
    return (uint16_t)(3u + byte_count);
}

/**
 * @brief  Handle an FC03/FC04 read-registers request: validate, copy
 *         the requested registers big-endian, and build the response.
 * @param fc        the function code (echoed into the response).
 * @param table     the register table to read, or NULL if absent.
 * @param table_len the table's register count.
 * @param resp      the response buffer to fill.
 * @return the PDU length written, or 0 to drop the request.
 */
static uint16_t handle_read_regs(uint8_t fc, const uint16_t *table, uint16_t table_len,
                                   uint8_t *resp)
{
    if (s_frame_len != 8u)
    {
        return 0u;
    }
    uint16_t start = be16(&s_frame[2]);
    uint16_t qty   = be16(&s_frame[4]);

    if (qty == 0u || qty > 125u)
    {
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_VALUE);
    }
    if (table == NULL || (uint32_t)start + qty > table_len)
    {
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }
    uint16_t byte_count = (uint16_t)(qty * 2u);
    if ((uint32_t)byte_count + 5u > EPIC_MODBUS_MAX_ADU)
    { /* +5 = addr+fc+bytecount+crc16 */
        return build_exception(resp, fc, MB_EXC_ILLEGAL_DATA_VALUE);
    }

    resp[0] = s_slave_addr;
    resp[1] = fc;
    resp[2] = (uint8_t)byte_count;
    for (uint16_t i = 0; i < qty; i++)
    {
        put_be16(&resp[3 + 2u * i], table[start + i]);
    }
    return (uint16_t)(3u + byte_count);
}

/**
 * @brief  Handle an FC05 write-single-coil request: validate the value,
 *         set the coil, and echo the request.
 * @param resp the response buffer to fill.
 * @return the PDU length written (6), or 0 to drop the request.
 */
static uint16_t handle_write_single_coil(uint8_t *resp)
{
    if (s_frame_len != 8u)
    {
        return 0u;
    }
    uint16_t addr  = be16(&s_frame[2]);
    uint16_t value = be16(&s_frame[4]);

    if (value != 0xFF00u && value != 0x0000u)
    {
        return build_exception(resp, MB_FC_WRITE_SINGLE_COIL, MB_EXC_ILLEGAL_DATA_VALUE);
    }
    if (s_map->coils == NULL || addr >= s_map->num_coils)
    {
        return build_exception(resp, MB_FC_WRITE_SINGLE_COIL, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }
    bit_set(s_map->coils, addr, value == 0xFF00u);

    /* success response echoes the request verbatim */
    resp[0] = s_slave_addr;
    resp[1] = MB_FC_WRITE_SINGLE_COIL;
    resp[2] = s_frame[2];
    resp[3] = s_frame[3];
    resp[4] = s_frame[4];
    resp[5] = s_frame[5];
    return 6u;
}

/**
 * @brief  Handle an FC06 write-single-register request: validate the
 *         address, store the value, and echo the request.
 * @param resp the response buffer to fill.
 * @return the PDU length written (6), or 0 to drop the request.
 */
static uint16_t handle_write_single_reg(uint8_t *resp)
{
    if (s_frame_len != 8u)
    {
        return 0u;
    }
    uint16_t addr  = be16(&s_frame[2]);
    uint16_t value = be16(&s_frame[4]);

    if (s_map->holding_regs == NULL || addr >= s_map->num_holding_regs)
    {
        return build_exception(resp, MB_FC_WRITE_SINGLE_REG, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }
    s_map->holding_regs[addr] = value;

    resp[0] = s_slave_addr;
    resp[1] = MB_FC_WRITE_SINGLE_REG;
    resp[2] = s_frame[2];
    resp[3] = s_frame[3];
    resp[4] = s_frame[4];
    resp[5] = s_frame[5];
    return 6u;
}

/**
 * @brief  Handle an FC0F write-multiple-coils request: validate the
 *         frame, write the coils, and build the response.
 * @param resp the response buffer to fill.
 * @return the PDU length written (6), or 0 to drop the request.
 */
static uint16_t handle_write_multiple_coils(uint8_t *resp)
{
    if (s_frame_len < 9u)
    { /* addr+fc+start(2)+qty(2)+bytecount(1)+>=1 data+crc(2) */
        return 0u;
    }
    uint16_t start      = be16(&s_frame[2]);
    uint16_t qty        = be16(&s_frame[4]);
    uint8_t  byte_count = s_frame[6];
    const uint8_t *data = &s_frame[7];

    if (qty == 0u || qty > 1968u || byte_count != (uint16_t)((qty + 7u) / 8u))
    {
        return build_exception(resp, MB_FC_WRITE_MULTIPLE_COILS, MB_EXC_ILLEGAL_DATA_VALUE);
    }
    if ((uint16_t)(7u + byte_count + 2u) != s_frame_len)
    {
        return 0u; /* declared byte count doesn't match the received frame length */
    }
    if (s_map->coils == NULL || (uint32_t)start + qty > s_map->num_coils)
    {
        return build_exception(resp, MB_FC_WRITE_MULTIPLE_COILS, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }

    for (uint16_t i = 0; i < qty; i++)
    {
        bool v = (bool)((data[i >> 3] >> (i & 7u)) & 1u);
        bit_set(s_map->coils, (uint16_t)(start + i), v);
    }

    resp[0] = s_slave_addr;
    resp[1] = MB_FC_WRITE_MULTIPLE_COILS;
    resp[2] = s_frame[2];
    resp[3] = s_frame[3];
    resp[4] = s_frame[4];
    resp[5] = s_frame[5];
    return 6u;
}

/**
 * @brief  Handle an FC10 write-multiple-registers request: validate the
 *         frame, store the registers, and build the response.
 * @param resp the response buffer to fill.
 * @return the PDU length written (6), or 0 to drop the request.
 */
static uint16_t handle_write_multiple_regs(uint8_t *resp)
{
    if (s_frame_len < 9u)
    {
        return 0u;
    }
    uint16_t start      = be16(&s_frame[2]);
    uint16_t qty        = be16(&s_frame[4]);
    uint8_t  byte_count = s_frame[6];
    const uint8_t *data = &s_frame[7];

    if (qty == 0u || qty > 123u || byte_count != (uint16_t)(qty * 2u))
    {
        return build_exception(resp, MB_FC_WRITE_MULTIPLE_REGS, MB_EXC_ILLEGAL_DATA_VALUE);
    }
    if ((uint16_t)(7u + byte_count + 2u) != s_frame_len)
    {
        return 0u;
    }
    if (s_map->holding_regs == NULL || (uint32_t)start + qty > s_map->num_holding_regs)
    {
        return build_exception(resp, MB_FC_WRITE_MULTIPLE_REGS, MB_EXC_ILLEGAL_DATA_ADDRESS);
    }

    for (uint16_t i = 0; i < qty; i++)
    {
        s_map->holding_regs[start + i] = be16(&data[2u * i]);
    }

    resp[0] = s_slave_addr;
    resp[1] = MB_FC_WRITE_MULTIPLE_REGS;
    resp[2] = s_frame[2];
    resp[3] = s_frame[3];
    resp[4] = s_frame[4];
    resp[5] = s_frame[5];
    return 6u;
}

/* RS-485 direction control + transmit */
/**
 * @brief  Transmit a response: assert the RS-485 driver enable (if
 *         configured), write the bytes, flush, and release the enable.
 * @param resp the response bytes to transmit (addr+fc+payload+CRC).
 * @param len  number of bytes in `resp`.
 */
static void send_response(const uint8_t *resp, uint16_t len)
{
    if (s_dir_configured)
    {
        EPIC_GPIO_WritePin((GPIO_TypeDef)s_dir_port, (uint16_t)EPIC_BIT(s_dir_pin), GPIO_PIN_SET);
    }
    epic_serial_write(resp, (int)len);
    if (s_dir_configured)
    {
        epic_serial_flush(); /* wait for the ring AND the shift register to drain
                                 before dropping the driver enable */
        EPIC_GPIO_WritePin((GPIO_TypeDef)s_dir_port, (uint16_t)EPIC_BIT(s_dir_pin), GPIO_PIN_RESET);
    }
}

/* frame validation + dispatch */
/**
 * @brief  Validate the accumulated frame (length, CRC-16, address match
 *         or broadcast) and dispatch it: build the response through the
 *         per-function-code handler and transmit it. Broadcast requests
 *         are applied but never answered.
 */
static void process_frame(void)
{
    if (s_frame_len < 4u)
    {
        return; /* shorter than addr+fc+crc16, can't be a real ADU */
    }

    uint16_t crc_calc = modbus_crc16(s_frame, (uint16_t)(s_frame_len - 2u));
    if (s_frame[s_frame_len - 2u] != (uint8_t)(crc_calc & 0xFFu) ||
        s_frame[s_frame_len - 1u] != (uint8_t)(crc_calc >> 8))
        {
        return; /* bad CRC, drop silently like every RTU slave does */
    }

    uint8_t addr      = s_frame[0];
    bool    broadcast = (addr == 0u);
    if (!broadcast && addr != s_slave_addr)
    {
        return; /* frame is for a different slave */
    }

    uint8_t  fc = s_frame[1];
    uint8_t  resp[EPIC_MODBUS_MAX_ADU];
    uint16_t pdu_len;

    switch (fc)
    {
    case MB_FC_READ_COILS:
        pdu_len = handle_read_bits(fc, s_map->coils, s_map->num_coils, resp);
        break;
    case MB_FC_READ_DISCRETE_INPUTS:
        pdu_len = handle_read_bits(fc, s_map->discrete_inputs, s_map->num_discrete_inputs, resp);
        break;
    case MB_FC_READ_HOLDING_REGS:
        pdu_len = handle_read_regs(fc, s_map->holding_regs, s_map->num_holding_regs, resp);
        break;
    case MB_FC_READ_INPUT_REGS:
        pdu_len = handle_read_regs(fc, s_map->input_regs, s_map->num_input_regs, resp);
        break;
    case MB_FC_WRITE_SINGLE_COIL:
        pdu_len = handle_write_single_coil(resp);
        break;
    case MB_FC_WRITE_SINGLE_REG:
        pdu_len = handle_write_single_reg(resp);
        break;
    case MB_FC_WRITE_MULTIPLE_COILS:
        pdu_len = handle_write_multiple_coils(resp);
        break;
    case MB_FC_WRITE_MULTIPLE_REGS:
        pdu_len = handle_write_multiple_regs(resp);
        break;
    default:
        pdu_len = build_exception(resp, fc, MB_EXC_ILLEGAL_FUNCTION);
        break;
    }

    if (pdu_len == 0u || broadcast)
    {
        return; /* malformed request, or a broadcast (processed above, never answered) */
    }

    uint16_t resp_crc = modbus_crc16(resp, pdu_len);
    resp[pdu_len]     = (uint8_t)(resp_crc & 0xFFu);
    resp[pdu_len + 1] = (uint8_t)(resp_crc >> 8);
    send_response(resp, (uint16_t)(pdu_len + 2u));
}

/* public API */
/**
 * @brief  Initialize the Modbus RTU slave (see the header for the full
 *         contract): configure the UART, precompute the T3.5 timeout,
 *         and store the address and register map.
 * @param fosc_hz    system oscillator frequency in Hz.
 * @param baud       RTU baud rate.
 * @param slave_addr this slave's Modbus address, 1..247.
 * @param map        the register map (stored by pointer, not copied).
 */
void epic_modbus_slave_init(uint32_t fosc_hz, uint32_t baud,
                             uint8_t slave_addr,
                             const epic_modbus_slave_map_t *map)
{
    epic_serial_init(fosc_hz, baud);

    s_slave_addr     = slave_addr;
    s_map            = map;
    s_t3_5_ms        = compute_t3_5_ms(baud);
    s_frame_len      = 0u;
    s_dir_configured = false;
}

/**
 * @brief  Configure the optional RS-485 driver-enable pin (see the
 *         header for the full contract): store it, set it as an output,
 *         and leave it low (idle = receive).
 * @param port HAL GPIO port index.
 * @param pin  bit index 0..7 within that port.
 */
void epic_modbus_slave_set_rs485_dir_pin(uint8_t port, uint8_t pin)
{
    s_dir_port       = port;
    s_dir_pin        = pin;
    s_dir_configured = true;

    EPIC_GPIO_Init((GPIO_TypeDef)port, (uint16_t)EPIC_BIT(pin), GPIO_MODE_OUTPUT);
    EPIC_GPIO_WritePin((GPIO_TypeDef)port, (uint16_t)EPIC_BIT(pin), GPIO_PIN_RESET); /* idle = receive */
}

/**
 * @brief  Poll the slave once (see the header for the full contract):
 *         drain newly received UART bytes into the frame buffer,
 *         dispatch once the T3.5 silence has elapsed, and reset the
 *         frame buffer after every dispatch attempt.
 */
void epic_modbus_slave_poll(void)
{
    int avail = epic_serial_available();
    if (avail > 0)
    {
        uint16_t space = (uint16_t)(EPIC_MODBUS_MAX_ADU - s_frame_len);
        if (space > 0u)
        {
            int n = epic_serial_read(&s_frame[s_frame_len], (int)space);
            if (n > 0)
            {
                s_frame_len = (uint16_t)(s_frame_len + (uint16_t)n);
            }
        }
        if ((uint16_t)avail > space)
        {
            /* frame already overflowed the ADU buffer: drain and discard the
             * rest so the RX ring never wedges. The oversized frame fails
             * length/CRC validation once silence is detected. */
            uint8_t  scratch[8];
            uint16_t remaining = (uint16_t)((uint16_t)avail - space);
            while (remaining > 0u)
            {
                int chunk = (remaining < 8u) ? (int)remaining : 8;
                int n     = epic_serial_read(scratch, chunk);
                if (n <= 0)
                {
                    break;
                }
                remaining = (uint16_t)(remaining - (uint16_t)n);
            }
        }
        s_last_rx_tick = epic_tick_get();
    }

    if (s_frame_len > 0u && epic_tick_elapsed_since(s_last_rx_tick) >= s_t3_5_ms)
    {
        process_frame();
        s_frame_len = 0u;
    }
}
