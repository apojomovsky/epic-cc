/*
 * Shared core for the PIC18F4550 bridge demo: a Modbus RTU slave
 * (UART) bridged to an MCP23017 I2C expander plus ADC AN0/AN1, run
 * by epic-taskmgr. One core, two entries (examples/ real hardware,
 * tests/ the mdb gate); only the stimulus source differs (live
 * UART/I2C/ADC vs the bridge_demo_inject_* sim seam).
 */

#ifndef BRIDGE_DEMO_CORE_H
#define BRIDGE_DEMO_CORE_H

#include <stdint.h>

/**
 * @brief Initialize tick, serial, ADC, I2C, expander handle, and map.
 *
 * Call once before spawning any epic-taskmgr task below. Issues no I2C
 * transaction itself; the first bridge task run programs the expander.
 */
void bridge_demo_init(void);

/**
 * @brief Stage one request byte for the sim-side Modbus responder.
 *
 * The sim stimulus seam: MPLAB SIM cannot deliver UART RX bytes to
 * firmware (RCREG writes ignored, RCIF masked), so scripted frames go
 * here instead of the serial ring. The bridge task answers them from
 * the same register arrays the live slave owns, through the real TX
 * path, after the same T3.5 silence the slave enforces.
 *
 * @param b the request byte to stage
 */
void bridge_demo_inject_rx_byte(uint8_t b);

/**
 * @brief Override one ADC channel with a scripted raw reading.
 *
 * The sim stimulus seam: once set for a channel, the ADC task samples
 * this value instead of starting real conversions (MPLAB SIM has no
 * analog stimulus, and each conversion costs a console warning there).
 *
 * @param channel 0 for AN0, 1 for AN1 (other values are ignored)
 * @param v the scripted raw ADC value, 0..1023
 */
void bridge_demo_inject_adc(uint8_t channel, uint16_t v);

/**
 * @brief Route expander traffic to a scripted register file.
 *
 * The sim I2C seam: the MSSP data path is unmodeled under MPLAB SIM
 * (SEN latches, SSPIF never sets), so a live MEM transaction would
 * hang the gate. After this call, reads/writes hit a static file
 * instead of the bus. Real hardware never calls it.
 */
void bridge_demo_use_sim_expander(void);

/**
 * @brief taskmgr task: answer staged frames, poll the slave, drive GPIO.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_bridge(void *arg);

/**
 * @brief taskmgr task: sample AN0/AN1, filter, publish into the map.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_adc(void *arg);

/**
 * @brief taskmgr task: UART heartbeat line with bridge state.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void bridge_demo_task_heartbeat(void *arg);

/**
 * @brief Filtered ADC reading in the 12-bit oversampled domain.
 * @param channel 0 for AN0, 1 for AN1
 * @return the latest averaged reading, 0..4092
 */
uint16_t bridge_demo_adc(uint8_t channel);

/**
 * @brief Last holding register image, for the sim oracle.
 * @param i register index, 0..5
 * @return the register value (0 when out of range)
 */
uint16_t bridge_demo_holding(uint8_t i);

/**
 * @brief Last input register image, for the sim oracle.
 * @param i register index, 0..3
 * @return the register value (0 when out of range)
 */
uint16_t bridge_demo_input(uint8_t i);

/**
 * @brief Combined expander output shadow, for the sim oracle.
 * @return GPB in the high byte, GPA in the low byte
 */
uint16_t bridge_demo_expander_shadow(void);

/**
 * @brief Staged-path responses transmitted so far, for the sim oracle.
 * @return the response count
 */
uint16_t bridge_demo_tx_frames(void);

/**
 * @brief Most recent Modbus exception code, for the sim oracle.
 * @return the code (0 when no exception has been sent yet)
 */
uint8_t bridge_demo_last_exception(void);

/**
 * @brief Logged staged-path response count, for the sim oracle.
 * @return the number of logged responses
 */
uint8_t bridge_demo_response_count(void);

/**
 * @brief Staged-frame buffer empty, for the sim stimulus pacing.
 *
 * Lets the script inject the next frame the moment the previous one
 * dispatched, instead of guessing a wall-clock gap (rounds run far
 * faster than simulated milliseconds under MPLAB SIM, so a fixed gap
 * in either unit would be wrong somewhere).
 *
 * @return nonzero when no staged bytes are pending dispatch
 */
uint8_t bridge_demo_staged_idle(void);

/**
 * @brief Copy one logged staged-path response, for the sim oracle.
 * @param idx response index, oldest first
 * @param buf destination for the response bytes
 * @param max capacity of buf
 * @return the logged length (0 when idx is out of range)
 */
uint8_t bridge_demo_response(uint8_t idx, uint8_t *buf, uint8_t max);

#endif /* BRIDGE_DEMO_CORE_H */
