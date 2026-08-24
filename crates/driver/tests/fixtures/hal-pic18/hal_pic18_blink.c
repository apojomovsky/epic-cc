/* Blink an LED on RB0, the PIC18 analog of the 887 canonical HAL smoke.
 * The sim has no timer hardware, so this program does not wait for a real
 * Timer0 overflow: it programs the port and timer, then in a bounded loop
 * polls INTCON<TMR0IF> (which the e2e test asserts to drive the blink),
 * clearing the flag and toggling RB0. `g_toggle_count` counts toggles.
 *
 * This is the epic-cc build of the HAL's example_blink.c, adapted to the
 * slice and the flag-poll idiom.
 */

#include "hal_pic18.h"
#include "hal_pic18_gpio.h"

/* Timer0 handle/enum (see hal_pic18_timer0.c). */
typedef struct {
    uint8_t Mode;
    uint8_t ClockSource;
    uint8_t ClockEdge;
    uint8_t Prescaler;
    uint8_t PrescalerAssigned;
    uint8_t ReloadValue;
} TIMER0_HandleTypeDef;

#define TIMER0_BITMODE_8BIT  0x1U
#define TIMER0_CLOCK_INTERNAL 0x0U
#define TIMER0_EDGE_RISING   0x0U
#define TIMER0_PRESCALER_1_256 0x7U

#define SIM_CYCLES 600000UL

extern void EPIC_GPIO_Init(GPIO_TypeDef port, uint16_t pins, GPIO_ModeTypeDef mode);
extern void EPIC_GPIO_WritePin(GPIO_TypeDef port, uint16_t pins, GPIO_PinState state);
extern void EPIC_GPIO_TogglePin(GPIO_TypeDef port, uint16_t pins);
extern EPIC_StatusTypeDef EPIC_TIMER0_Init(const TIMER0_HandleTypeDef *h);
extern EPIC_StatusTypeDef EPIC_TIMER0_Start(const TIMER0_HandleTypeDef *h);
extern void EPIC_IRQ_Restore(uint8_t prev_state);
extern void TIMER0_IRQHandler(void);

/* Toggle count, the blink loop is the only writer. Non-static so
 * the e2e test can read it from the address map. */
volatile uint32_t g_toggle_count = 0;

static void on_t0_overflow(void)
{
    EPIC_GPIO_TogglePin(GPIOB, GPIO_PIN_0);
    g_toggle_count++;
}

int main(void)
{
    /* 1. RB0 as output, start low. */
    EPIC_GPIO_Init(GPIOB, GPIO_PIN_0, GPIO_MODE_OUTPUT);
    EPIC_GPIO_WritePin(GPIOB, GPIO_PIN_0, GPIO_PIN_RESET);

    /* 2. Timer0: internal Fosc/4, 1:256 prescaler, reload 0. Built by
     * field assignment (no const-handle global). */
    TIMER0_HandleTypeDef h;
    h.Mode = TIMER0_BITMODE_8BIT;
    h.ClockSource = TIMER0_CLOCK_INTERNAL;
    h.ClockEdge = TIMER0_EDGE_RISING;
    h.Prescaler = TIMER0_PRESCALER_1_256;
    h.PrescalerAssigned = 1u;
    h.ReloadValue = 0x00U;
    EPIC_TIMER0_Init(&h);
    EPIC_TIMER0_Start(&h);

    /* 3. Arm the master interrupt enable. */
    EPIC_IRQ_Restore(1);

    /* 4. Bounded loop: pump the blink, driven by TMR0IF which the e2e
     * asserts. */
    for (uint32_t i = 0u; i < SIM_CYCLES; i++) {
        if (EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_TMR0IF) {
            EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_TMR0IF);
            on_t0_overflow();
        }
    }

    return (int)g_toggle_count;
}
