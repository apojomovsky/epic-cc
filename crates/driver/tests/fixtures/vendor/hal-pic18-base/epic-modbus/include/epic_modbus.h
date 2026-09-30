/* Family-agnostic Modbus RTU slave on epic-serial (UART), epic-tick
 * (T3.5 silence timing), and the HAL's GPIO (optional RS-485 driver
 * enable). RTU only, slave role only, FC 01-06/15/16. Register access
 * is plain caller-owned
 * arrays (epic_modbus_slave_map_t); poll() dispatches once the T3.5
 * inter-frame gap elapses. */

#ifndef EPIC_MODBUS_H
#define EPIC_MODBUS_H

#include <stdint.h>

/** Max RTU ADU (address + PDU + CRC) this module buffers. Override before
 *  including this header, e.g. -DEPIC_MODBUS_MAX_ADU=128, to fit a larger
 *  register map's read/write-multiple responses. */
#ifndef EPIC_MODBUS_MAX_ADU
#define EPIC_MODBUS_MAX_ADU 64u
#endif

/**
 * @brief  The slave's register map: caller-owned storage, this module
 *         only reads/writes through these pointers, never allocates.
 *         Coils/discrete_inputs are bit-packed (LSB of array[0] =
 *         address 0); holding/input regs are one uint16_t per address.
 *         A NULL array (count 0) makes that table absent: matching
 *         requests get an ILLEGAL DATA ADDRESS exception.
 */
typedef struct {
    uint8_t        *coils;              /**< bit-packed, read/write        */
    uint16_t        num_coils;
    const uint8_t  *discrete_inputs;    /**< bit-packed, read-only         */
    uint16_t        num_discrete_inputs;
    uint16_t       *holding_regs;       /**< read/write                    */
    uint16_t        num_holding_regs;
    const uint16_t *input_regs;         /**< read-only                     */
    uint16_t        num_input_regs;
} epic_modbus_slave_map_t;

/**
 * @brief  Initialize the Modbus RTU slave: configures the UART (via
 *         `epic_serial_init`) at @p baud, precomputes the RTU T3.5
 *         inter-frame timeout for that baud, and stores @p slave_addr and
 *         @p map. Call once at startup, after `epic_tick_init`.
 * @param  fosc_hz     system oscillator frequency in Hz.
 * @param  baud        RTU baud rate, e.g. 9600. Also drives the T3.5 timing,
 *                     see README.md for the >19200 baud caveat.
 * @param  slave_addr  this slave's Modbus address, 1..247. 0 is reserved
 *                     for broadcast requests, do not pass it here.
 * @param  map         the register map (see @ref epic_modbus_slave_map_t).
 *                     The pointer is stored, not copied, keep it alive for
 *                     the lifetime of the program.
 */
void epic_modbus_slave_init(uint32_t fosc_hz, uint32_t baud,
                             uint8_t slave_addr,
                             const epic_modbus_slave_map_t *map);

/**
 * @brief  Configure an optional RS-485 driver-enable pin, asserted for the
 *         duration of each response transmission. If never called, TX goes
 *         out through `epic_serial_write` untouched (fine for point-to-point
 *         wiring or an auto-sensing transceiver).
 * @param  port  HAL GPIO port index (cast internally to the family's
 *               `GPIO_TypeDef`, e.g. `GPIOB` is port index 1).
 * @param  pin   bit index 0..7 within that port.
 */
void epic_modbus_slave_set_rs485_dir_pin(uint8_t port, uint8_t pin);

/**
 * @brief  Poll the slave once. Call every main-loop iteration (or wire as a
 *         `epic-taskmgr` task). Drains newly received UART bytes into the
 *         frame buffer; once the RTU T3.5 silence has elapsed since the
 *         last received byte, validates the frame (length, CRC-16, address
 *         match or broadcast) and, if valid, dispatches it and transmits
 *         the response (broadcast requests never get a response). The
 *         frame buffer is reset after every dispatch attempt, valid or not,
 *         so the next byte always starts a fresh frame.
 */
void epic_modbus_slave_poll(void);

#endif /* EPIC_MODBUS_H */
