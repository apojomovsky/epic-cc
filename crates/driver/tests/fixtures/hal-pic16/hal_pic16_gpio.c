/* Vendored slice of the real epic-hal pic16f87xa-hal GPIO driver
 * (DS39582B §4.2), adapted to compile under epic-cc. Reduced to the
 * blink surface: RB0 as output, toggle. The PORTB change callback
 * (RB_IRQHandler) is the 1-arg cross-context shape that exercises the
 * arity-filtered indirect-call candidate set (epic-cc#152).
 */

#include "hal_pic16_gpio.h"
#include <stddef.h>

/* One callback slot for the whole-port RB<7:0> change interrupt. */
static void (*s_rb_change_callback)(uint8_t) = NULL;

/**
 * @brief Configure a port pin's direction.
 * @param port GPIOA..GPIOE.
 * @param pins Bitmask of GPIO_PIN_0 .. GPIO_PIN_All.
 * @param mode GPIO_MODE_OUTPUT (TRIS bit 0) or input.
 */
void EPIC_GPIO_Init(GPIO_TypeDef port, uint16_t pins, GPIO_ModeTypeDef mode)
{
    (void)port;
    if (mode == GPIO_MODE_OUTPUT) {
        EPIC_REG8(PIC_REG_TRISB) &= (uint8_t)~(uint8_t)pins;
    } else {
        EPIC_REG8(PIC_REG_TRISB) |= (uint8_t)pins;
    }
}

/**
 * @brief Write a pin level through the latch.
 * @param port GPIOA..GPIOE.
 * @param pins Bitmask of GPIO_PIN_0 .. GPIO_PIN_All.
 * @param state GPIO_PIN_SET to drive high, GPIO_PIN_RESET for low.
 */
void EPIC_GPIO_WritePin(GPIO_TypeDef port, uint16_t pins, GPIO_PinState state)
{
    (void)port;
    uint8_t v = EPIC_REG8(PIC_REG_PORTB);
    if (state == GPIO_PIN_SET) {
        EPIC_REG8(PIC_REG_PORTB) = (uint8_t)(v | (uint8_t)pins);
    } else {
        EPIC_REG8(PIC_REG_PORTB) = (uint8_t)(v & (uint8_t)~(uint8_t)pins);
    }
}

/**
 * @brief Toggle a pin through the latch.
 * @param port GPIOA..GPIOE.
 * @param pins Bitmask of GPIO_PIN_0 .. GPIO_PIN_All.
 */
void EPIC_GPIO_TogglePin(GPIO_TypeDef port, uint16_t pins)
{
    (void)port;
    EPIC_REG8(PIC_REG_PORTB) ^= (uint8_t)pins;
}

/**
 * @brief Install or remove the PORTB change callback (param-forwarded
 *        registration, epic-cc#137 cross-context shape).
 * @param callback function called with the PORTB byte on an RB<7:0>
 *        change, or NULL to unregister.
 */
void EPIC_GPIO_RegisterChangeCallback(void (*callback)(uint8_t))
{
    s_rb_change_callback = callback;
}

/**
 * @brief Weak RB<7:0> change ISR: reads PORTB first, clears RBIF, then
 *        fires the registered callback with the byte.
 */
void RB_IRQHandler(void)
{
    /* MUST read PORTB before clearing RBIF (DS39582B §14.11.3). */
    if (!(EPIC_REG8(PIC_REG_INTCON) & PIC_INTCON_RBIF)) return;
    uint8_t portb = EPIC_REG8(PIC_REG_PORTB);
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_INTCON), PIC_INTCON_RBIF);
    if (s_rb_change_callback) s_rb_change_callback(portb);
}
