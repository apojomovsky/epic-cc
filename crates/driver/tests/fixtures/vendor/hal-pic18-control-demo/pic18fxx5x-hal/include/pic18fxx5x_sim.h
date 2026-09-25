/*
 * Public API for the PIC18F2455 family host simulation backend. On the
 * host build every SFR access indexes a host-side register file
 * (`include/host/pic18_platform.h`); these hooks let the host application
 * drive input pins, read output-pin levels, advance simulated
 * peripherals by N instruction cycles, and register an interrupt
 * callback. Names mirror `pic16f87xa_sim_*` for symmetry with the PIC16
 * tree.
 */

#ifndef PIC18FXX5X_SIM_H
#define PIC18FXX5X_SIM_H

#include <stdint.h>
#include "pic18fxx5x.h"

/**
 * @brief Initialize the simulation backend and reset every SFR to its
 *        power-on-reset value (DS39632E Table 5-1). Must be called before
 *        any HAL call.
 */
void pic18_sim_reset(void);

/**
 * @brief Advance the simulated peripherals by `ticks` instruction cycles.
 *        Drives Timer0 (and raises TMR0IF + the IRQ callback on overflow).
 * @param ticks the number of instruction cycles to simulate.
 */
void pic18_sim_step(uint32_t ticks);

/**
 * @brief Drive a digital input pin from the test rig (when the pin is
 *        configured as input, TRIS bit = 1).
 *
 * @param port   One of 'A'..'E'.
 * @param pin    Pin number 0..7.
 * @param level  0 = low, 1 = high.
 */
void pic18_sim_drive_input(char port, uint8_t pin, uint8_t level);

/**
 * @brief Read the level currently driven onto an output pin (what an
 *        external load would see). For pins configured as outputs, this
 *        returns the latched value in LATx (DS39632E §10.0). For inputs,
 *        this returns the last value driven via @ref pic18_sim_drive_input.
 * @param port the port letter, 'A'..'E'.
 * @param pin the pin number, 0..7.
 * @return the level on the pin: 0 = low, 1 = high.
 */
uint8_t pic18_sim_read_output(char port, uint8_t pin);

/**
 * @brief Hook a user callback fired whenever the simulated CPU would take
 *        an interrupt. The host harness registers the family dispatcher
 *        here.
 */
typedef void (*pic18_sim_irq_cb_t)(void);

/**
 * @brief Hook a user callback fired whenever the simulated CPU would take
 *        an interrupt. The host harness registers the family dispatcher
 *        here.
 * @param cb the callback to invoke on every simulated interrupt, or NULL
 *        to unregister.
 */
void pic18_sim_set_irq_callback(pic18_sim_irq_cb_t cb);

/**
 * @brief Inject a byte into the MSSP receiver (SPI slave or I2C target).
 *        Stores `data` in SSPBUF, sets SSPSTAT<BF>, and raises SSPIF
 *        (PIR1<3>). The next EPIC_SSP_ReadByte() returns it.
 * @param data the byte to inject into the MSSP receiver.
 */
void pic18_sim_drive_ssp_rx(uint8_t data);

/**
 * @brief Inject a byte into the EUSART receiver. Stores `data` in RCREG
 *        and raises RCIF (PIR1<5>). The next EPIC_USART_Receive() returns
 *        it. Mirrors pic16f87xa_sim_drive_usart_rx().
 * @param data the byte to inject into the EUSART receiver.
 */
void pic18_sim_drive_usart_rx(uint8_t data);

/**
 * @brief Drive the comparator outputs from the test rig. Sets CMCON<C1OUT>
 *        and/or CMCON<C2OUT> and raises CMIF (PIR2<6>) to model a change of
 *        output level. The next EPIC_COMP_C1Out()/C2Out() reads reflect it.
 * @param c1out  0 or 1 for the C1 output level.
 * @param c2out  0 or 1 for the C2 output level.
 */
void pic18_sim_drive_comp(uint8_t c1out, uint8_t c2out);

/**
 * @brief Preload the simulated EEPROM cell at `addr` with `data` (model a
 *        byte already stored in the part). The next EPIC_EEPROM_ReadByte()
 *        returns it.
 * @param addr the EEPROM address, 0..255.
 * @param data the byte to preload.
 */
void pic18_sim_drive_eeprom_byte(uint8_t addr, uint8_t data);

/**
 * @brief Model an EEPROM write completion: stores `data` at `addr` in the
 *        simulated EEPROM array and raises EEIF (PIR2<4>). The next read
 *        returns the value just written.
 * @param addr the EEPROM address, 0..255.
 * @param data the byte written by the simulated EEPROM.
 */
void pic18_sim_drive_eeprom_done(uint8_t addr, uint8_t data);

/**
 * @brief Read the simulated EEPROM cell at `addr` (host backend for
 *        EPIC_EEPROM_ReadByte). `addr` is a uint8_t (0..255), always a
 *        valid index into the 256-byte array.
 * @param addr the EEPROM address, 0..255.
 * @return the byte stored at `addr`.
 */
uint8_t pic18_sim_eeprom_read(uint8_t addr);

/**
 * @brief Model an A/D conversion completion: clears ADCON0<GO/DONE>, stores
 *        the 10-bit `result` into ADRESH:ADRESL in the format selected by
 *        ADCON2<ADFM> (right- or left-justified), and raises ADIF (PIR1<6>).
 *        The next EPIC_ADC_Read() returns the value (un-justified to 0..1023).
 * @param result the 10-bit A/D result, 0..1023.
 */
void pic18_sim_drive_adc_done(uint16_t result);

#if PIC18FXX5X_FAMILY_HAS_SPP
/**
 * @brief Model an SPP transfer event: sets the SPPEPS<WRSPP>/<RDSPP>
 *        status bits (as selected by `wrspp`/`rdspp`) and raises SPPIF
 *        (PIR1<7>). 40/44-pin parts only.
 * @param wrspp 1 to set the WRSPP status bit, else 0.
 * @param rdspp 1 to set the RDSPP status bit, else 0.
 */
void pic18_sim_drive_spp(uint8_t wrspp, uint8_t rdspp);
#endif

#endif /* PIC18FXX5X_SIM_H */
