/*
 * epic-encoder target example: x4 quadrature decode on the RB<7:4>
 * interrupt-on-change, position logged over serial. Wire channel A to
 * RB4 and channel B to RB5; see docs/API.md for the port-byte bit
 * convention and the at-most-two-encoders-per-port ceiling.
 */

#include "encoder.h"
#include "epic_tick.h"
#include "epic_serial.h"
#include "epic_hal.h"

/* XC8 streams printf through putch and needs the header; epic-cc has no
 * stdio and epic-serial replaces printf with its literal shim instead. */
#ifndef __EPIC_CC__
 #include <stdio.h>
#endif

/* The IRQ source enum is family-specific; the build defines PIC<mcu>,
 * same pattern epic-serial uses for its TX/RX IRQ ids. */
#if defined(PIC18F2455) || defined(PIC18F2550) || defined(PIC18F4455) || defined(PIC18F4550)
#define EXAMPLE_IRQ_RB PIC18_IRQ_RB
#else
#define EXAMPLE_IRQ_RB PIC16_IRQ_RB
#endif

#define ENC_PIN_A      4u      /* RB4 */
#define ENC_PIN_B      5u      /* RB5 */
#define GLITCH_GATE_MS 5u      /* min edge interval; 0 disables the gate */
#define LOG_PERIOD_MS  250u

static epic_encoder_t g_encoder;

/**
 * @brief RB-change callback: forward the freshly-read PORTB byte to the decoder.
 */
static void on_rb_change(uint8_t portb_value)
{
    epic_encoder_update(&g_encoder, portb_value);
}

/**
 * @brief Decode x4 quadrature on RB<7:4> and log position over serial.
 */
int main(void)
{
    epic_tick_init(FOSC_HZ);
    epic_serial_init(FOSC_HZ, 115200u);
    EPIC_IRQ_Restore(1);

    EPIC_GPIO_Init(GPIOB, GPIO_PIN_4 | GPIO_PIN_5, GPIO_MODE_INPUT);
    EPIC_GPIO_RegisterChangeCallback(on_rb_change);
    epic_encoder_init(&g_encoder, ENC_PIN_A, ENC_PIN_B, GLITCH_GATE_MS,
                      EPIC_GPIO_ReadPort(GPIOB));
    EPIC_IRQ_Enable(EXAMPLE_IRQ_RB);

    printf("epic-encoder: x4 quadrature on RB4/RB5, position logged\r\n");

    uint32_t last_log = epic_tick_get();
    for (;;) {
        if (epic_tick_elapsed_since(last_log) >= LOG_PERIOD_MS) {
            last_log = epic_tick_get();
            /* Value-only put_* composition: the epic-cc printf shim is
             * literal-only, and the put_* forms render the same bytes
             * on both toolchains. */
            epic_serial_put_str("pos=");
            epic_serial_put_i32(epic_encoder_get_position(&g_encoder));
            epic_serial_put_str(" err=");
            epic_serial_put_u16(epic_encoder_get_error_count(&g_encoder));
            epic_serial_put_str(" glitch=");
            epic_serial_put_u16(epic_encoder_get_glitch_count(&g_encoder));
            epic_serial_put_str("\r\n");
        }
    }
}
