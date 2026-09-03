/**
 * Vendor-agnostic, interrupt-driven incremental quadrature decoder (x4),
 * one `epic_encoder_t` per A/B channel pair. Software-decoded via the RB<7:4>
 * interrupt-on-change source (neither family has a QEI peripheral): wire
 * A/B to two of RB4-RB7 and forward the port byte to `epic_encoder_update()`
 * from the HAL's RB-change callback. See docs/API.md for wiring.
 */

#ifndef ENCODER_H
#define ENCODER_H

#include <stdint.h>

/**
 * One quadrature decoder instance, caller-owned storage: one per A/B
 * channel pair (at most two per PORTB, see docs/API.md).
 *
 * `position` is written only from ISR context. `error_count` (impossible
 * Gray transitions) and `glitch_count` (edges rejected by the interval
 * gate) stay separate because each diagnoses a different failure mode.
 */
typedef struct {
    uint8_t           pin_a;                /**< bit position 0-7 of channel A in the port byte. */
    uint8_t           pin_b;                /**< bit position 0-7 of channel B in the port byte. */
    volatile int32_t  position;
    uint8_t           last_state;           /**< 2-bit (a<<1|b), previous sample. */
    uint16_t          min_edge_interval_ms; /**< 0 = glitch gate disabled. */
    uint32_t          last_edge_tick;        /**< epic_tick_get() at the last accepted edge;
                                                  *  meaningless when the gate is disabled. */
    volatile uint16_t error_count;
    volatile uint16_t glitch_count;
} epic_encoder_t;

/**
 * @brief Initialize an encoder instance.
 *
 * Swapping `pin_a`/`pin_b` inverts count direction (see docs/API.md).
 *
 * @param enc                  the encoder instance to initialize
 * @param pin_a                bit position 0-7 of channel A in the port byte
 * @param pin_b                bit position 0-7 of channel B in the port byte
 * @param min_edge_interval_ms 0 disables the glitch gate; otherwise two
 *                             edges closer than this (ms) drop the second
 * @param port_value           current port byte; seeds `last_state` so the
 *                             first real edge isn't misjudged
 */
void epic_encoder_init(epic_encoder_t *enc, uint8_t pin_a, uint8_t pin_b,
                  uint16_t min_edge_interval_ms, uint8_t port_value);

/**
 * @brief Re-sync an encoder instance from a fresh port sample.
 *
 * Re-syncs `last_state` from `port_value` and zeroes `position`,
 * `error_count`, `glitch_count`, `last_edge_tick`; pins and
 * `min_edge_interval_ms` unchanged. For fault recovery without re-wiring
 * the instance.
 *
 * @param enc         the encoder instance to reset
 * @param port_value  current port byte; re-seeds `last_state`
 */
void epic_encoder_reset(epic_encoder_t *enc, uint8_t port_value);

/**
 * @brief Decode one port sample.
 *
 * Call from the application's RB-change callback, once per registered
 * instance, with the received byte. No-ops if this instance's 2-bit
 * state didn't change; drops a too-soon edge as a glitch when the
 * interval gate is armed; an impossible Gray transition increments
 * `error_count` and leaves `position` unchanged.
 *
 * @param enc         the encoder instance to update
 * @param port_value  the port byte just sampled
 */
void epic_encoder_update(epic_encoder_t *enc, uint8_t port_value);

/**
 * @brief Read the accumulated position atomically.
 *
 * The ISR updates it as a multi-byte RMW, so the read retries until two
 * consecutive reads agree (no tear on an 8-bit core).
 *
 * @param enc the encoder instance to read
 * @return the current position count
 */
int32_t  epic_encoder_get_position(const epic_encoder_t *enc);

/**
 * @brief Read the impossible-transition counter atomically.
 *
 * Same retry as epic_encoder_get_position.
 *
 * @param enc the encoder instance to read
 * @return the current error count
 */
uint16_t epic_encoder_get_error_count(const epic_encoder_t *enc);

/**
 * @brief Read the rejected-by-gate counter atomically.
 *
 * Same retry as epic_encoder_get_position (see @ref epic_encoder_get_error_count).
 *
 * @param enc the encoder instance to read
 * @return the current glitch count
 */
uint16_t epic_encoder_get_glitch_count(const epic_encoder_t *enc);

#endif /* ENCODER_H */
