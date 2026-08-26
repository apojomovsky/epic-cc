/* GPIO surface of the vendored hal-pic16 slice: port/pin/mode types and
 * the RB change callback API. Shared by the driver TU and the blink TU
 * so prototypes and definitions agree (an i8/i16 mismatch in a call arg
 * panics isel's narrow-arg widening). */

#ifndef HAL_PIC16_GPIO_H
#define HAL_PIC16_GPIO_H

#include "hal_pic16.h"

typedef enum {
    GPIOA = 0,
    GPIOB = 1,
    GPIOC = 2,
    GPIOD = 3,
    GPIOE = 4
} GPIO_TypeDef;

#define GPIO_PIN_0 EPIC_BIT(0)

typedef enum {
    GPIO_PIN_RESET = 0U,
    GPIO_PIN_SET   = 1U
} GPIO_PinState;

typedef enum {
    GPIO_MODE_OUTPUT = 0x2U
} GPIO_ModeTypeDef;

void EPIC_GPIO_Init(GPIO_TypeDef port, uint16_t pins, GPIO_ModeTypeDef mode);
void EPIC_GPIO_WritePin(GPIO_TypeDef port, uint16_t pins, GPIO_PinState state);
void EPIC_GPIO_TogglePin(GPIO_TypeDef port, uint16_t pins);
void EPIC_GPIO_RegisterChangeCallback(void (*callback)(uint8_t));

#endif /* HAL_PIC16_GPIO_H */
