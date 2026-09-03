/* Public API for the PIC16F87XA host simulation backend: drive input
 * pins, read output-pin levels, advance time via pic16f87xa_sim_step(),
 * and inject peripheral events. SFRs index the host-side register file
 * (include/host/pic16f87xa_platform.h); see src/sim/pic16f87xa_sim.c
 * for the peripheral models. */

#ifndef PIC16F87XA_SIM_H
#define PIC16F87XA_SIM_H

#include <stdint.h>
#include <stdbool.h>
#include "pic16f87xa.h"

/**
 * @brief Initialize the simulation backend and reset every SFR to its
 *        power-on-reset value. Must be called before any HAL call.
 */
void pic16f87xa_sim_reset(void);

/**
 * @brief Advance the simulated peripherals by `ticks` instruction cycles.
 *        Drives Timer0, A/D, etc.
 * @param ticks the number of instruction cycles to advance.
 */
void pic16f87xa_sim_step(uint32_t ticks);

/**
 * @brief Drive a digital input pin from the test rig (when the pin is
 *        configured as input, TRIS bit = 1).
 *
 * @param port   One of 'A'..'E'.
 * @param pin    Pin number 0..7.
 * @param level  0 = low, 1 = high.
 */
void pic16f87xa_sim_drive_input(char port, uint8_t pin, uint8_t level);

/**
 * @brief Read the level currently driven onto an output pin (what an
 *        external load would see). For pins configured as outputs, this
 *        returns the latched value in PORTx. For inputs, this returns
 *        the last value driven via @ref pic16f87xa_sim_drive_input.
 * @param port One of 'A'..'E'.
 * @param pin Pin number 0..7.
 * @return the driven pin level, 0 or 1.
 */
uint8_t pic16f87xa_sim_read_output(char port, uint8_t pin);

/**
 * @brief Hook a user callback fired whenever the simulated CPU would take
 *        an interrupt. Lets tests assert "an interrupt should fire here".
 */
typedef void (*pic16f87xa_sim_irq_cb_t)(void);

/**
 * @brief Install or remove the interrupt callback.
 * @param cb the callback to fire on a simulated interrupt, or NULL to
 *        unregister.
 */
void pic16f87xa_sim_set_irq_callback(pic16f87xa_sim_irq_cb_t cb);

/**
 * @brief Inject a byte into the USART receiver as if it had just been
 *        received off the wire. Sets PIR1<RCIF> and stores the byte in
 *        RCREG. The next call to EPIC_USART_Receive() will return it.
 * @param data the byte to inject.
 */
void pic16f87xa_sim_drive_usart_rx(uint8_t data);

/**
 * @brief Inject a byte into the SSP receiver (SPI slave or I²C target).
 *        Sets SSPSTAT<BF> and PIR1<SSPIF>. The next call to
 *        EPIC_SSP_ReadByte() will return the byte.
 * @param data the byte to inject.
 */
void pic16f87xa_sim_drive_ssp_rx(uint8_t data);

/**
 * @brief Drive an A/D conversion to completion with a given 10-bit
 *        result. Stores `result` in ADRESH:ADRESL (right-justified),
 *        clears GO/DONE, and sets PIR1<ADIF>. The next call to
 *        EPIC_ADC_Read() will return the result.
 * @param result the 10-bit conversion result, 0..1023.
 */
void pic16f87xa_sim_drive_adc_done(uint16_t result);

/**
 * @brief Place a byte in the simulated EEPROM. Subsequent calls to
 *        EPIC_EEPROM_ReadByte(addr) return it.
 * @param addr the EEPROM address, 0..255.
 * @param data the byte to store.
 */
void pic16f87xa_sim_drive_eeprom_byte(uint8_t addr, uint8_t data);

/**
 * @brief Simulate a completed EEPROM write at `addr` with `data` and
 *        set PIR2<EEIF>.
 * @param addr the EEPROM address that was written.
 * @param data the byte that was stored.
 */
void pic16f87xa_sim_drive_eeprom_done(uint8_t addr, uint8_t data);

#endif /* PIC16F87XA_SIM_H */
