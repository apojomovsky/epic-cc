/** 4-bit parallel GPIO transport for epic_lcd; R/W tied low. */

#include "epic_lcd_transport.h"
#include "epic_hal.h"
#include "epic_tick.h"

typedef struct {
    epic_lcd_gpio4_pins_t pins;
} gpio4_ctx_t;

/**
 * @brief Send one byte over the 4-bit parallel bus.
 *
 * Drives RS, writes the high nibble, pulses E, writes the low nibble,
 * pulses E again (HD44780 4-bit protocol).
 *
 * @param ctx  transport context (gpio4_ctx_t)
 * @param rs   register select: 0 = instruction, 1 = data
 * @param byte byte to send
 */
static void gpio4_send(void *ctx, uint8_t rs, uint8_t byte)
{
    gpio4_ctx_t *g = (gpio4_ctx_t *)ctx;

    EPIC_GPIO_WritePin(g->pins.rs_port, g->pins.rs_pin,
                      rs ? GPIO_PIN_SET : GPIO_PIN_RESET);

    /* High nibble first (HD44780 4-bit protocol) */
    uint8_t hi = (uint8_t)(byte >> 4u);
    EPIC_GPIO_WritePin(g->pins.db4_port, g->pins.db4_pin,
                      (hi & 0x01u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db5_port, g->pins.db5_pin,
                      (hi & 0x02u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db6_port, g->pins.db6_pin,
                      (hi & 0x04u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db7_port, g->pins.db7_pin,
                      (hi & 0x08u) ? GPIO_PIN_SET : GPIO_PIN_RESET);

    /* E pulse: high then low */
    EPIC_GPIO_WritePin(g->pins.e_port, g->pins.e_pin, GPIO_PIN_SET);
    EPIC_GPIO_WritePin(g->pins.e_port, g->pins.e_pin, GPIO_PIN_RESET);

    uint8_t lo = (uint8_t)(byte & 0x0Fu);
    EPIC_GPIO_WritePin(g->pins.db4_port, g->pins.db4_pin,
                      (lo & 0x01u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db5_port, g->pins.db5_pin,
                      (lo & 0x02u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db6_port, g->pins.db6_pin,
                      (lo & 0x04u) ? GPIO_PIN_SET : GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g->pins.db7_port, g->pins.db7_pin,
                      (lo & 0x08u) ? GPIO_PIN_SET : GPIO_PIN_RESET);

    EPIC_GPIO_WritePin(g->pins.e_port, g->pins.e_pin, GPIO_PIN_SET);
    EPIC_GPIO_WritePin(g->pins.e_port, g->pins.e_pin, GPIO_PIN_RESET);
}

/**
 * @brief Delay a number of microseconds using epic-tick.
 *
 * epic-tick's resolution is 1 ms; sub-ms delays are a best-effort busy
 * wait. Fine for HD44780 commands, since the timing is a minimum, not
 * exact.
 *
 * @param ctx transport context (unused)
 * @param us  microseconds to wait
 */
static void gpio4_delay_us(void *ctx, uint32_t us)
{
    (void)ctx;
    if (us >= 1000u)
    {
        epic_tick_delay_ms(us / 1000u);
    }
    /* Sub-ms: no precise timer on 8-bit PIC without Timer intervention;
     * the E-pulse setup/hold time is already satisfied by the
     * EPIC_GPIO_WritePin call overhead (several us at 20-48 MHz). */
}

/**
 * @brief Delay a number of milliseconds using epic-tick.
 *
 * @param ctx transport context (unused)
 * @param ms  milliseconds to wait
 */
static void gpio4_delay_ms(void *ctx, uint32_t ms)
{
    (void)ctx;
    epic_tick_delay_ms(ms);
}

/**
 * @brief Initialize the 4-bit parallel GPIO transport.
 *
 * Configures the RS/E/DB4-DB7 pins as outputs, asserts E low, and binds
 * the transport's send/delay ops into ops. No special cold-start
 * handling: the 4-bit init preamble (0x3, 0x3, 0x3, then 0x28) falls out
 * of epic_lcd_init's three 0x38 sends, since the LCD only reads the first
 * nibble of each 0x3X send while still in 8-bit mode.
 *
 * @param ops   transport ops struct to fill in
 * @param ctx   receives the transport context pointer to pass to ops calls
 * @param pins  GPIO pin assignments for RS, E, and DB4-DB7
 */
void epic_lcd_gpio4_init(epic_lcd_ops_t *ops, void **ctx,
                         const epic_lcd_gpio4_pins_t *pins)
{
    static gpio4_ctx_t g;
    g.pins = *pins;

    EPIC_GPIO_Init(g.pins.rs_port,  g.pins.rs_pin,  GPIO_MODE_OUTPUT);
    EPIC_GPIO_Init(g.pins.e_port,   g.pins.e_pin,   GPIO_MODE_OUTPUT);
    EPIC_GPIO_Init(g.pins.db4_port, g.pins.db4_pin,  GPIO_MODE_OUTPUT);
    EPIC_GPIO_Init(g.pins.db5_port, g.pins.db5_pin,  GPIO_MODE_OUTPUT);
    EPIC_GPIO_Init(g.pins.db6_port, g.pins.db6_pin,  GPIO_MODE_OUTPUT);
    EPIC_GPIO_Init(g.pins.db7_port, g.pins.db7_pin,  GPIO_MODE_OUTPUT);

    EPIC_GPIO_WritePin(g.pins.e_port, g.pins.e_pin, GPIO_PIN_RESET);
    EPIC_GPIO_WritePin(g.pins.rs_port, g.pins.rs_pin, GPIO_PIN_RESET);

    ops->send     = gpio4_send;
    ops->delay_us = gpio4_delay_us;
    ops->delay_ms = gpio4_delay_ms;
    *ctx = &g;
}
