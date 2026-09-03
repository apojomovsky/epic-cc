/**
 * Status codes and bit macros shared, unmodified, by every 8-bit PIC
 * HAL family; mirrors STM32Cube's `HAL_StatusTypeDef` so consumer code
 * never sees a family-specific status enum or bit macro.
 */

#ifndef EPIC_STATUS_H
#define EPIC_STATUS_H

#include <stdint.h>

typedef enum {
    EPIC_OK      = 0x00U,
    EPIC_ERROR   = 0x01U,
    EPIC_BUSY    = 0x02U,
    EPIC_TIMEOUT = 0x03U,
    EPIC_INVALID = 0x04U
} EPIC_StatusTypeDef;

#define EPIC_BIT(n)                  (1U << (n))
#define EPIC_BIT_SET(reg, mask)     ((reg) |=  (uint8_t)(mask))
#define EPIC_BIT_CLR(reg, mask)     ((reg) &= ~(uint8_t)(mask))
#define EPIC_BIT_TGL(reg, mask)      ((reg) ^=  (uint8_t)(mask))
#define EPIC_BIT_READ(reg, mask)     ((reg) &   (uint8_t)(mask))

#endif /* EPIC_STATUS_H */
