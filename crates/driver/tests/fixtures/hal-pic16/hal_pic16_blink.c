/* Callback-driven blink on RB0 (epic-hal#105 acceptance): the headline
 * HAL API under epic-cc. main registers a Timer0 overflow callback
 * through the inlined Init (the store folds to a named literal,
 * ADR-024), registers an RB change callback through the param-forwarded
 * shape, reads the const `irq_table` through EPIC_IRQ_GetFlagMask (the
 * non-zero regression guard, epic-cc#114), then idles. The e2e fires
 * the Timer0 interrupt and asserts the callback ran end to end.
 */

#include "hal_pic16.h"
#include "hal_pic16_timer0.h"
#include "hal_pic16_gpio.h"

/* Callback/irq entry points (weak handlers, dispatch from the vector). */
void TIMER0_IRQHandler(void);
void RB_IRQHandler(void);
void EPIC_IRQ_Restore(uint8_t prev_state);
uint8_t EPIC_IRQ_GetFlagMask(uint8_t irq);

/* Toggle count, the ISR callback is the only writer. */
volatile uint32_t g_toggle_count = 0;
/* RB callback latch, written by the RB ISR callback with the PORTB byte. */
volatile uint8_t g_rb_seen = 0;
/* Runtime index for the irq_table field read (epic-cc#114 shape). */
volatile uint8_t g_irq_idx = 2;
/* irq_table field read-back, written from main after init. */
volatile uint8_t g_irq_readback = 0;

static void on_t0_overflow(void)
{
    EPIC_GPIO_TogglePin(1u, 1u); /* GPIOB, GPIO_PIN_0 */
    g_toggle_count++;
}

static void on_rb_change(uint8_t portb)
{
    g_rb_seen = portb;
}

void main(void)
{
    /* 1. RB0 as output, start low. */
    EPIC_GPIO_Init(1u, 1u, 0x2u);
    EPIC_GPIO_WritePin(1u, 1u, 0u);

    /* 2. Timer0: internal Fosc/4, 1:256 prescaler, reload 0, toggle on
     *    each overflow. Local handle: Init/Start are header inlines, so
     *    clang folds the callback store to a named literal and epic-cc
     *    resolves the ISR dispatch (ADR-024). */
    TIMER0_HandleTypeDef h = TIMER0_HANDLE_DEFAULT;
    h.ClockSource       = 0x0U; /* TIMER0_CLOCK_INTERNAL */
    h.Prescaler         = 0x7U; /* TIMER0_PRESCALER_1_256 */
    h.PrescalerAssigned = 1u;
    h.ReloadValue       = 0x00U;
    h.OverflowCallback  = on_t0_overflow;
    EPIC_TIMER0_Init(&h);
    EPIC_TIMER0_Start(&h);

    /* 3. Register the RB change callback (param-forwarded shape) and arm
     *    the master enable. */
    EPIC_GPIO_RegisterChangeCallback(on_rb_change);
    EPIC_IRQ_Restore(1);

    /* 4. Read the const irq_table through a runtime index (epic-cc#114
     *    regression shape): the TMR0 descriptor's flag bit mask is
     *    INTCON<TMR0IF>, non-zero in the initializer bytes. A zero blob
     *    reads 0. The index is volatile so the table read cannot be
     *    constant-folded away. */
    g_irq_readback = EPIC_IRQ_GetFlagMask(g_irq_idx);

    /* 5. Idle forever; the e2e fires the TMR0 interrupt and asserts the
     *    callback ran. */
    for (;;) {
    }
}
