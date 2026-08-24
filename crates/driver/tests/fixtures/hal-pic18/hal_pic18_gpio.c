/* Vendored slice of the real epic-hal pic18fxx5x-hal GPIO driver for the
 * PIC18F4550 (DS39632E §10.0), adapted to compile under epic-cc. Reduced
 * to the blink surface: RB0 as an output, toggled through the LATx latch.
 *
 * Adapted for the epic-cc backend, matching the 887 HAL's epic-cc GPIO
 * variant: no runtime-computed SFR address (a `uint16_t` returned by a
 * helper lowers to `inttoptr i16 %reg`, which isel-pic18 does not
 * resolve) and no LLVM `switch` (irparse does not lower it). Each port
 * branch reads/writes its literal `EPIC_REG8(PIC_REG_*)` address directly.
 */

#include "hal_pic18.h"
#include "hal_pic18_gpio.h"

/* Implemented pins per port. */
static uint8_t port_width(GPIO_TypeDef port)
{
    if (port == GPIOA) return 6U;
    return 8U;
}

void EPIC_GPIO_Init(GPIO_TypeDef port, uint16_t pins, GPIO_ModeTypeDef mode)
{
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);

    if (mode == GPIO_MODE_OUTPUT) {
        /* Clear the direction bit (output). */
        if (port == GPIOA) {
            uint8_t t = EPIC_REG8(0xF92U); EPIC_REG8(0xF92U) = (uint8_t)(t & (uint8_t)~mask);
        } else if (port == GPIOB) {
            uint8_t t = EPIC_REG8(PIC_REG_TRISB); EPIC_REG8(PIC_REG_TRISB) = (uint8_t)(t & (uint8_t)~mask);
        } else {
            uint8_t t = EPIC_REG8(0xF94U); EPIC_REG8(0xF94U) = (uint8_t)(t & (uint8_t)~mask);
        }
    } else if (mode == GPIO_MODE_INPUT || mode == GPIO_MODE_ANALOG) {
        /* Set the direction bit (input). */
        if (port == GPIOA) {
            uint8_t t = EPIC_REG8(0xF92U); EPIC_REG8(0xF92U) = (uint8_t)(t | mask);
        } else if (port == GPIOB) {
            uint8_t t = EPIC_REG8(PIC_REG_TRISB); EPIC_REG8(PIC_REG_TRISB) = (uint8_t)(t | mask);
        } else {
            uint8_t t = EPIC_REG8(0xF94U); EPIC_REG8(0xF94U) = (uint8_t)(t | mask);
        }
    }
}

void EPIC_GPIO_WritePin(GPIO_TypeDef port, uint16_t pins, GPIO_PinState state)
{
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);

    if (port == GPIOA) {
        uint8_t cur = EPIC_REG8(0xF89U); /* LATA */
        if (state == GPIO_PIN_SET) cur |= mask; else cur &= (uint8_t)~mask;
        EPIC_REG8(0xF89U) = cur;
    } else if (port == GPIOB) {
        uint8_t cur = EPIC_REG8(PIC_REG_LATB);
        if (state == GPIO_PIN_SET) cur |= mask; else cur &= (uint8_t)~mask;
        EPIC_REG8(PIC_REG_LATB) = cur;
    } else {
        uint8_t cur = EPIC_REG8(0xF8BU); /* LATC */
        if (state == GPIO_PIN_SET) cur |= mask; else cur &= (uint8_t)~mask;
        EPIC_REG8(0xF8BU) = cur;
    }
}

void EPIC_GPIO_TogglePin(GPIO_TypeDef port, uint16_t pins)
{
    uint8_t mask = (uint8_t)pins & (uint8_t)((1U << port_width(port)) - 1U);

    if (port == GPIOA) {
        EPIC_REG8(0xF89U) = (uint8_t)(EPIC_REG8(0xF89U) ^ mask);
    } else if (port == GPIOB) {
        EPIC_REG8(PIC_REG_LATB) = (uint8_t)(EPIC_REG8(PIC_REG_LATB) ^ mask);
    } else {
        EPIC_REG8(0xF8BU) = (uint8_t)(EPIC_REG8(0xF8BU) ^ mask);
    }
}
