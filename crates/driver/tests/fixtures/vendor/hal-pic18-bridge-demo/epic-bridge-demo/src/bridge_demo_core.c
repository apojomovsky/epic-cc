/*
 * Bridge demo core: Modbus RTU slave bridged to MCP23017 GPIOs and
 * ADC AN0/AN1 under epic-taskmgr. Holding regs drive the expander,
 * the ADC task publishes readings, heartbeats report state. See
 * bridge_demo_core.h for why sim stimulus differs from live wiring.
 */

#include "bridge_demo_core.h"

#include "epic_hal.h"
#include "epic_adcfilter.h"
#include "epic_bus.h"
#include "epic_mcp23x17.h"
#include "epic_modbus.h"
#include "epic_serial.h"
#include "epic_taskmgr.h"
#include "epic_tick.h"

#include <stdint.h>

#ifndef FOSC_HZ
#define FOSC_HZ 48000000UL
#endif

#define SLAVE_ADDR  1U
#define MODBUS_BAUD 9600U
#define EXP_ADDR    0x20U

/* Register images. Holding 0/1 are the GPA/GPB output images (low
 * byte each); 2/3 carry the filtered ADC readings; 4 is the taskmgr
 * tick; 5 is status (bit15 expander error latch, low byte last
 * exception). Input 0/1 mirror the ADC readings, 2 the tick, 3 the
 * status. Live ADC/tick values overwrite whatever a master writes
 * to 2..5 on the next publish, by design. */
#define HOLD_N      6U
#define HOLD_GPA    0U
#define HOLD_GPB    1U
#define HOLD_ADC0   2U
#define HOLD_ADC1   3U
#define HOLD_TICK   4U
#define HOLD_STATUS 5U
#define INPUT_N     4U
#define IN_ADC0     0U
#define IN_ADC1     1U
#define IN_TICK     2U
#define IN_STATUS   3U

#define OVERSAMPLE_BITS 2U
#define AVG_WINDOW      4U

/* Staged-frame seam: the demo only answers fixed 8-byte read/write
 * requests, so the staging buffer holds exactly one. Longer arrivals
 * set the overrun flag and the frame is dropped at silence time. */
#define INJ_SZ 8U

/* Logged staged responses: FC03 over all six holding regs is the
 * longest (addr+fc+count+12 data+crc = 17), and the gate scripts at
 * most six answering frames. */
#define RESP_N  6U
#define RESP_SZ 17U

/* Scripted expander file: IODIR 0x00/0x01, GPIO 0x11/0x12, OLAT
 * 0x13/0x14, so 0x00..0x14 covers every access this demo issues. */
#define SIMREGS_N  21U
#define REG_IODIR  0x00U
#define REG_GPIO   0x11U
#define REG_OLAT   0x13U

/* T3.5 silence on the shared 1 ms epic_tick timebase (Timer2):
 * ceil(3.5 x 11 x 1000 / 9600) = 5 ms, the same ceiling
 * epic_modbus_slave_init computes, so the seam and the live slave
 * agree on what a frame boundary is. MPLAB SIM advances Timer2
 * with the instruction stream; a free-running Timer1 does not. */
#define T35_MS 5U

#define FC_READ_HOLDING 0x03U
#define FC_READ_INPUT   0x04U
#define FC_WRITE_REG    0x06U
#define EXC_FUNC        0x01U
#define EXC_ADDR        0x02U
#define EXC_VALUE       0x03U

/* One bridge block instead of scattered tiny globals: the linker's
 * small-data pools are nearly full on this part, so a single
 * multi-byte symbol packs into a banked pool and leaves access RAM
 * for symbols that cannot move (same reason as the control demo). */
static struct {
    uint16_t holding[HOLD_N];
    uint16_t input[INPUT_N];
    uint16_t avg0_buf[AVG_WINDOW];
    uint16_t avg1_buf[AVG_WINDOW];
    uint8_t inj_buf[INJ_SZ];
    uint8_t resp[RESP_N][RESP_SZ];
    uint8_t tx_buf[RESP_SZ];
    uint8_t sim_regs[SIMREGS_N];
    uint8_t resp_len[RESP_N];
    uint16_t sim_adc0;
    uint16_t sim_adc1;
    uint16_t adc0;
    uint16_t adc1;
    uint16_t tx_frames;
    uint32_t inj_stamp;
    uint8_t inj_len;
    uint8_t inj_overrun;
    uint8_t resp_count;
    uint8_t last_exc;
    uint8_t shadow_a;
    uint8_t shadow_b;
    uint8_t sim0_valid;
    uint8_t sim1_valid;
    uint8_t exp_cfg;
    uint8_t i2c_err;
    epic_adcfilter_avg_t avg0;
    epic_adcfilter_avg_t avg1;
    epic_mcp23x17_handle_t exp;
    epic_mcp23x17_transport_t sim_transport;
    epic_modbus_slave_map_t map;
} g;

/**
 * @brief CRC-16/Modbus over buf (poly 0xA001, init 0xFFFF, bit loop).
 * @param buf bytes to checksum
 * @param len number of bytes in buf
 * @return the CRC-16 value
 */
static uint16_t crc16(const uint8_t *buf, uint8_t len)
{
    uint16_t crc = 0xFFFFU;
    uint8_t i;
    uint8_t bit;
    for (i = 0U; i < len; i++)
    {
        crc = (uint16_t)(crc ^ buf[i]);
        for (bit = 0U; bit < 8U; bit++)
        {
            if (crc & 1U)
            {
                crc = (uint16_t)((crc >> 1) ^ 0xA001U);
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
 * @brief Read a big-endian uint16_t from a byte pair.
 * @param p the two bytes, high byte first
 * @return the decoded value
 */
static uint16_t be16(const uint8_t *p)
{
    return (uint16_t)(((uint16_t)p[0] << 8) | p[1]);
}

/**
 * @brief Write a uint16_t as a big-endian byte pair.
 * @param p destination pair, high byte first
 * @param v the value to write
 */
static void put_be16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)v;
}

/**
 * @brief Take one raw AN0 sample, or the sim override when injected.
 * @param ctx unused (epic_adcfilter_read_fn signature)
 * @return one raw 10-bit sample
 */
static uint16_t adc_read_an0(void *ctx)
{
    uint16_t v;
    (void)ctx;
    if (g.sim0_valid)
    {
        return g.sim_adc0;
    }
    EPIC_ADC_SelectChannel(ADC_CHANNEL_AN0);
    (void)EPIC_ADC_Start();
    while (EPIC_ADC_IsConversionInProgress())
    {
        /* Bounded by hardware: acquisition plus conversion is a
         * handful of Tad, far shorter than the ADC period. */
    }
    v = EPIC_ADC_Read();
    EPIC_ADC_ClearITFlag();
    return v;
}

/**
 * @brief Take one raw AN1 sample, or the sim override when injected.
 * @param ctx unused (epic_adcfilter_read_fn signature)
 * @return one raw 10-bit sample
 */
static uint16_t adc_read_an1(void *ctx)
{
    uint16_t v;
    (void)ctx;
    if (g.sim1_valid)
    {
        return g.sim_adc1;
    }
    EPIC_ADC_SelectChannel(ADC_CHANNEL_AN1);
    (void)EPIC_ADC_Start();
    while (EPIC_ADC_IsConversionInProgress())
    {
        /* Bounded by hardware, same as the AN0 reader above. */
    }
    v = EPIC_ADC_Read();
    EPIC_ADC_ClearITFlag();
    return v;
}

/**
 * @brief Scripted-file read: serve register bytes from the static file.
 * @param ctx unused (the file is core static, needs no context)
 * @param reg first register address
 * @param buf destination for the bytes
 * @param n number of bytes to read
 * @return n on success, -1 when the range leaves the file
 */
static int sim_reg_read(void *ctx, uint8_t reg, uint8_t *buf, int n)
{
    int i;
    (void)ctx;
    if (n <= 0 || (uint16_t)reg + (uint16_t)n > SIMREGS_N)
    {
        return -1;
    }
    for (i = 0; i < n; i++)
    {
        buf[i] = g.sim_regs[(uint8_t)((uint8_t)reg + (uint8_t)i)];
    }
    return n;
}

/**
 * @brief Scripted-file write: store bytes, mirroring GPIO into OLAT.
 *
 * The demo keeps every pin an output, where a GPIO read returns the
 * latch, so each stored GPIO byte is copied to its OLAT twin. That is
 * the whole model: no interrupts, no polarity, nothing the demo
 * touches.
 *
 * @param ctx unused (the file is core static, needs no context)
 * @param reg first register address
 * @param buf the bytes to store
 * @param n number of bytes to write
 * @return n on success, -1 when the range leaves the file
 */
static int sim_reg_write(void *ctx, uint8_t reg, const uint8_t *buf, int n)
{
    int i;
    uint8_t r;
    (void)ctx;
    if (n <= 0 || (uint16_t)reg + (uint16_t)n > SIMREGS_N)
    {
        return -1;
    }
    for (i = 0; i < n; i++)
    {
        r = (uint8_t)((uint8_t)reg + (uint8_t)i);
        g.sim_regs[r] = buf[i];
        if (r == REG_GPIO || r == (uint8_t)(REG_GPIO + 1U))
        {
            g.sim_regs[(uint8_t)(r + 2U)] = buf[i];
        }
    }
    return n;
}

/**
 * @brief Transmit a staged PDU: append CRC, send, log, count.
 * @param pdu_len bytes staged in g.tx_buf (addr+fc+payload)
 */
static void staged_send(uint8_t pdu_len)
{
    uint16_t crc;
    uint8_t i;
    uint8_t total;
    crc = crc16(g.tx_buf, pdu_len);
    g.tx_buf[pdu_len] = (uint8_t)(crc & 0xFFU);
    g.tx_buf[(uint8_t)(pdu_len + 1U)] = (uint8_t)(crc >> 8);
    total = (uint8_t)(pdu_len + 2U);
    epic_serial_write(g.tx_buf, (int)total);
    if (g.resp_count < RESP_N)
    {
        for (i = 0U; i < total; i++)
        {
            g.resp[g.resp_count][i] = g.tx_buf[i];
        }
        g.resp_len[g.resp_count] = total;
        g.resp_count++;
    }
    g.tx_frames++;
}

/**
 * @brief Answer a staged request with an exception frame.
 * @param fc the requested function code (echoed with the top bit set)
 * @param exc the exception code to report
 */
static void staged_exception(uint8_t fc, uint8_t exc)
{
    g.tx_buf[0] = SLAVE_ADDR;
    g.tx_buf[1] = (uint8_t)(fc | 0x80U);
    g.tx_buf[2] = exc;
    g.last_exc = exc;
    staged_send(3U);
}

/**
 * @brief Answer a staged FC03/FC04 read from one register table.
 *
 * Validation mirrors epic-modbus exactly (same codes for the same
 * faults), so the seam and the live slave can never disagree about
 * what a frame means.
 *
 * @param fc the function code being answered
 * @param table the register image to read
 * @param table_len the image length in registers
 */
static void staged_read(uint8_t fc, const uint16_t *table, uint8_t table_len)
{
    uint16_t start;
    uint16_t qty;
    uint16_t i;
    uint8_t n;
    if (g.inj_len != 8U)
    {
        return;
    }
    start = be16(&g.inj_buf[2]);
    qty = be16(&g.inj_buf[4]);
    if (qty == 0U || qty > 125U)
    {
        staged_exception(fc, EXC_VALUE);
        return;
    }
    if ((uint32_t)start + qty > table_len)
    {
        staged_exception(fc, EXC_ADDR);
        return;
    }
    n = (uint8_t)(qty * 2U);
    g.tx_buf[0] = SLAVE_ADDR;
    g.tx_buf[1] = fc;
    g.tx_buf[2] = n;
    for (i = 0U; i < qty; i++)
    {
        put_be16(&g.tx_buf[(uint8_t)(3U + 2U * i)], table[(uint8_t)(start + i)]);
    }
    staged_send((uint8_t)(3U + n));
}

/**
 * @brief Answer a staged FC06 write into the holding image.
 */
static void staged_write_single(void)
{
    uint16_t addr;
    uint16_t value;
    if (g.inj_len != 8U)
    {
        return;
    }
    addr = be16(&g.inj_buf[2]);
    value = be16(&g.inj_buf[4]);
    if (addr >= HOLD_N)
    {
        staged_exception(FC_WRITE_REG, EXC_ADDR);
        return;
    }
    g.holding[(uint8_t)addr] = value;
    g.tx_buf[0] = SLAVE_ADDR;
    g.tx_buf[1] = FC_WRITE_REG;
    g.tx_buf[2] = g.inj_buf[2];
    g.tx_buf[3] = g.inj_buf[3];
    g.tx_buf[4] = g.inj_buf[4];
    g.tx_buf[5] = g.inj_buf[5];
    staged_send(6U);
}

/**
 * @brief Validate one staged frame and dispatch it, like process_frame.
 *
 * CRC/address/length faults drop silently per the RTU rule; function
 * faults answer with exceptions. Broadcast writes apply to the map
 * without any response, exactly as the live slave treats them.
 */
static void staged_dispatch(void)
{
    uint16_t crc;
    uint8_t addr;
    uint8_t fc;
    uint8_t len;
    len = g.inj_len;
    if (g.inj_overrun || len < 4U)
    {
        return;
    }
    crc = crc16(g.inj_buf, (uint8_t)(len - 2U));
    if (g.inj_buf[(uint8_t)(len - 2U)] != (uint8_t)(crc & 0xFFU) ||
        g.inj_buf[(uint8_t)(len - 1U)] != (uint8_t)(crc >> 8))
    {
        return;
    }
    addr = g.inj_buf[0];
    fc = g.inj_buf[1];
    if (addr == 0U)
    {
        if (fc == FC_WRITE_REG && len == 8U && be16(&g.inj_buf[2]) < HOLD_N)
        {
            g.holding[(uint8_t)be16(&g.inj_buf[2])] = be16(&g.inj_buf[4]);
        }
        return;
    }
    if (addr != SLAVE_ADDR)
    {
        return;
    }
    switch (fc)
    {
    case FC_READ_HOLDING:
        staged_read(fc, g.holding, HOLD_N);
        break;
    case FC_READ_INPUT:
        staged_read(fc, g.input, INPUT_N);
        break;
    case FC_WRITE_REG:
        staged_write_single();
        break;
    default:
        staged_exception(fc, EXC_FUNC);
        break;
    }
}

/**
 * @brief Dispatch staged bytes once the T3.5 silence has elapsed.
 *
 * The stamp is an epic_tick 1 ms timestamp, so the elapsed check is
 * wraparound-safe by subtraction, and the check costs one 32-bit
 * read per bridge run whether or not any byte was ever staged.
 */
static void staged_service(void)
{
    if (g.inj_len == 0U && g.inj_overrun == 0U)
    {
        return;
    }
    if (epic_tick_elapsed_since(g.inj_stamp) < T35_MS)
    {
        return;
    }
    staged_dispatch();
    g.inj_len = 0U;
    g.inj_overrun = 0U;
}

/**
 * @brief Program the expander once, then follow holding 0/1 on change.
 *
 * Direction and outputs go out on the first run, not in init, so a
 * missing device fails here rather than hanging init. Writes fire
 * only on change. Status is n-or-minus-1, like epic_bus_i2c_mem_write.
 */
static void expander_sync(void)
{
    int st;
    uint8_t a;
    uint8_t b;
    if (g.exp_cfg == 0U)
    {
        st = EPIC_MCP23X17_SetDirectionAll(&g.exp, 0x0000U);
        if (st < 0)
        {
            g.i2c_err = 1U;
        }
        g.exp_cfg = 1U;
    }
    a = (uint8_t)(g.holding[HOLD_GPA] & 0xFFU);
    if (a != g.shadow_a)
    {
        st = EPIC_MCP23X17_WritePort(&g.exp, EPIC_MCP23X17_PORTA, a);
        if (st < 0)
        {
            g.i2c_err = 1U;
        }
        else
        {
            g.shadow_a = a;
        }
    }
    b = (uint8_t)(g.holding[HOLD_GPB] & 0xFFU);
    if (b != g.shadow_b)
    {
        st = EPIC_MCP23X17_WritePort(&g.exp, EPIC_MCP23X17_PORTB, b);
        if (st < 0)
        {
            g.i2c_err = 1U;
        }
        else
        {
            g.shadow_b = b;
        }
    }
}

/**
 * @brief Initialize tick, serial, ADC, I2C, expander handle, and map.
 *
 * The tick timebase doubles as the staged-frame T3.5 clock, so no
 * extra timer is configured here. Call once before spawning any
 * epic-taskmgr task below.
 */
void bridge_demo_init(void)
{
    ADC_HandleTypeDef adc = ADC_HANDLE_DEFAULT;
    epic_tick_init(FOSC_HZ);
    epic_serial_init(FOSC_HZ, MODBUS_BAUD);
    adc.Channel   = ADC_CHANNEL_AN0;
    adc.PinConfig = 0x0U; /* all AN pins analog; AN0/AN1 sampled */
    (void)EPIC_ADC_Init(&adc);
    epic_bus_i2c_init(FOSC_HZ, 100000UL);
    (void)EPIC_MCP23X17_Init(&g.exp, EPIC_MCP23X17_BUS_I2C, EXP_ADDR);
    /* Field-by-field, not an aggregate literal: the implied zero-fill
     * lowers to a memset intrinsic epic-cc cannot legalize (the same
     * gap the menu demo documents for its LCD and CCP handles). */
    g.map.coils               = NULL;
    g.map.num_coils           = 0U;
    g.map.discrete_inputs     = NULL;
    g.map.num_discrete_inputs = 0U;
    g.map.holding_regs        = g.holding;
    g.map.num_holding_regs    = HOLD_N;
    g.map.input_regs          = g.input;
    g.map.num_input_regs      = INPUT_N;
    epic_modbus_slave_init(FOSC_HZ, MODBUS_BAUD, SLAVE_ADDR, &g.map);
    /* Static buffers outliving their filters: each average state keeps
     * its pointer for the life of the program, so stack buffers here
     * would dangle on return. */
    epic_adcfilter_avg_init(&g.avg0, g.avg0_buf, AVG_WINDOW);
    epic_adcfilter_avg_init(&g.avg1, g.avg1_buf, AVG_WINDOW);
}

/**
 * @brief Stage one request byte (see bridge_demo_core.h).
 * @param b the request byte to stage
 */
void bridge_demo_inject_rx_byte(uint8_t b)
{
    if (g.inj_len < INJ_SZ)
    {
        g.inj_buf[g.inj_len] = b;
        g.inj_len++;
    }
    else
    {
        g.inj_overrun = 1U;
    }
    g.inj_stamp = epic_tick_get();
}

/**
 * @brief Override one ADC channel (see bridge_demo_core.h).
 * @param channel 0 for AN0, 1 for AN1 (other values are ignored)
 * @param v the scripted raw ADC value, 0..1023
 */
void bridge_demo_inject_adc(uint8_t channel, uint16_t v)
{
    if (channel == 0U)
    {
        g.sim_adc0 = v;
        g.sim0_valid = 1U;
    }
    else if (channel == 1U)
    {
        g.sim_adc1 = v;
        g.sim1_valid = 1U;
    }
    else
    {
        /* Only two channels exist; anything else is a script bug, and
         * quietly ignoring it would hide that, but there is no error
         * channel on this seam, so ignoring is the documented shape. */
    }
}

/**
 * @brief Route expander traffic to a scripted file (see header).
 */
void bridge_demo_use_sim_expander(void)
{
    uint8_t i;
    for (i = 0U; i < SIMREGS_N; i++)
    {
        g.sim_regs[i] = 0U;
    }
    /* Reset defaults: IODIR powers up all-input, like the real part. */
    g.sim_regs[REG_IODIR] = 0xFFU;
    g.sim_regs[(uint8_t)(REG_IODIR + 1U)] = 0xFFU;
    /* Runtime assigns, not a literal: see the map comment in init. */
    g.sim_transport.read_reg = sim_reg_read;
    g.sim_transport.write_reg = sim_reg_write;
    g.sim_transport.ctx = NULL;
    (void)EPIC_MCP23X17_InitTransport(&g.exp, &g.sim_transport);
}

/**
 * @brief taskmgr task: answer staged frames, poll slave, drive GPIO.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_bridge(void *arg)
{
    uint16_t status;
    (void)arg;
    staged_service();
    epic_modbus_slave_poll();
    expander_sync();
    status = (uint16_t)g.last_exc;
    if (g.i2c_err)
    {
        status = (uint16_t)(status | 0x8000U);
    }
    g.holding[HOLD_STATUS] = status;
    g.input[IN_STATUS] = status;
    g.holding[HOLD_TICK] = epic_taskmgr_ticks();
    g.input[IN_TICK] = epic_taskmgr_ticks();
}

/**
 * @brief taskmgr task: sample AN0/AN1, filter, publish into the map.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_adc(void *arg)
{
    uint16_t raw0;
    uint16_t raw1;
    (void)arg;
    raw0 = epic_adcfilter_oversample(adc_read_an0, NULL, OVERSAMPLE_BITS);
    g.adc0 = epic_adcfilter_avg_push(&g.avg0, raw0);
    raw1 = epic_adcfilter_oversample(adc_read_an1, NULL, OVERSAMPLE_BITS);
    g.adc1 = epic_adcfilter_avg_push(&g.avg1, raw1);
    g.holding[HOLD_ADC0] = g.adc0;
    g.holding[HOLD_ADC1] = g.adc1;
    g.input[IN_ADC0] = g.adc0;
    g.input[IN_ADC1] = g.adc1;
}

/**
 * @brief taskmgr task: UART heartbeat line with bridge state.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_heartbeat(void *arg)
{
    uint16_t shadow;
    (void)arg;
    shadow = (uint16_t)(((uint16_t)g.shadow_b << 8) | g.shadow_a);
    epic_serial_put_str("HB addr=");
    epic_serial_put_u16((uint16_t)SLAVE_ADDR);
    epic_serial_put_str(" tx=");
    epic_serial_put_u16(g.tx_frames);
    epic_serial_put_str(" exc=");
    epic_serial_put_hex8(g.last_exc);
    epic_serial_put_str(" adc0=");
    epic_serial_put_u16(g.adc0);
    epic_serial_put_str(" adc1=");
    epic_serial_put_u16(g.adc1);
    epic_serial_put_str(" exp=");
    epic_serial_put_hex16(shadow);
    epic_serial_put_str("\n");
}

/**
 * @brief Filtered ADC reading (see bridge_demo_core.h).
 * @param channel 0 for AN0, 1 for AN1
 * @return the latest averaged reading, 0..4092
 */
uint16_t bridge_demo_adc(uint8_t channel)
{
    if (channel == 0U)
    {
        return g.adc0;
    }
    if (channel == 1U)
    {
        return g.adc1;
    }
    return 0U;
}

/**
 * @brief Holding register image (see bridge_demo_core.h).
 * @param i register index, 0..5
 * @return the register value (0 when out of range)
 */
uint16_t bridge_demo_holding(uint8_t i)
{
    if (i < HOLD_N)
    {
        return g.holding[i];
    }
    return 0U;
}

/**
 * @brief Input register image (see bridge_demo_core.h).
 * @param i register index, 0..3
 * @return the register value (0 when out of range)
 */
uint16_t bridge_demo_input(uint8_t i)
{
    if (i < INPUT_N)
    {
        return g.input[i];
    }
    return 0U;
}

/**
 * @brief Expander output shadow (see bridge_demo_core.h).
 * @return GPB in the high byte, GPA in the low byte
 */
uint16_t bridge_demo_expander_shadow(void)
{
    return (uint16_t)(((uint16_t)g.shadow_b << 8) | g.shadow_a);
}

/**
 * @brief Staged-path response count (see bridge_demo_core.h).
 * @return the response count
 */
uint16_t bridge_demo_tx_frames(void)
{
    return g.tx_frames;
}

/**
 * @brief Most recent exception code (see bridge_demo_core.h).
 * @return the code (0 when no exception has been sent yet)
 */
uint8_t bridge_demo_last_exception(void)
{
    return g.last_exc;
}

/**
 * @brief Logged response count (see bridge_demo_core.h).
 * @return the number of logged responses
 */
uint8_t bridge_demo_response_count(void)
{
    return g.resp_count;
}

/**
 * @brief Copy one logged response (see bridge_demo_core.h).
 * @param idx response index, oldest first
 * @param buf destination for the response bytes
 * @param max capacity of buf
 * @return the logged length (0 when idx is out of range)
 */
uint8_t bridge_demo_response(uint8_t idx, uint8_t *buf, uint8_t max)
{
    uint8_t len;
    uint8_t i;
    if (idx >= g.resp_count)
    {
        return 0U;
    }
    len = g.resp_len[idx];
    for (i = 0U; i < len && i < max; i++)
    {
        buf[i] = g.resp[idx][i];
    }
    return len;
}

/**
 * @brief Staged-frame buffer empty (see bridge_demo_core.h).
 * @return nonzero when no staged bytes are pending dispatch
 */
uint8_t bridge_demo_staged_idle(void)
{
    if (g.inj_len == 0U && g.inj_overrun == 0U)
    {
        return 1U;
    }
    return 0U;
}
