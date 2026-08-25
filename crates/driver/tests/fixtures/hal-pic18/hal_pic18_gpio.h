/* GPIO types for the PIC18F4550 smoke slice. Shared by the driver and the
 * fixture programs so call signatures agree on widths (enums are 2-byte
 * `int` on this target, per clang's -target msp430 proxy). */

#ifndef HAL_PIC18_GPIO_H
#define HAL_PIC18_GPIO_H

typedef enum {
    GPIOA = 0,
    GPIOB = 1,
    GPIOC = 2
} GPIO_TypeDef;

#define GPIO_PIN_0 EPIC_BIT(0)
#define GPIO_PIN_All 0xFFU

typedef enum {
    GPIO_PIN_RESET = 0U,
    GPIO_PIN_SET   = 1U
} GPIO_PinState;

typedef enum {
    GPIO_MODE_INPUT  = 0x1U,
    GPIO_MODE_OUTPUT = 0x2U,
    GPIO_MODE_ANALOG = 0x3U
} GPIO_ModeTypeDef;

#endif /* HAL_PIC18_GPIO_H */
