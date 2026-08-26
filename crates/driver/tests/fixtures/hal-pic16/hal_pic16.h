/* Vendored slice of the real epic-hal pic16f87xa-hal for the PIC16F877A,
 * adapted to compile and run under epic-cc. This is the epic-cc variant of
 * the HAL's SFR/GPIO/Timer0/IRQ layer, reduced to the slice the callback
 * e2e needs: GPIO on RB0, Timer0 with a cross-context overflow callback,
 * the IRQ controller (const irq_table), the interrupt vector and the
 * dispatch fan-out.
 *
 * The slice mirrors the guard-free HAL shape from epic-hal#105: the
 * callback is stored by main through the inlined EPIC_TIMER0_Init (the
 * header static inline folds the store to a named literal, ADR-024) and
 * fired from TIMER0_IRQHandler via the global the ISR reads. This is the
 * cross-context pattern epic-cc#137 acceptance covers.
 *
 * The e2e test drives simulated time by asserting the timer flag register
 * exactly as the hal-pic18 slice does: the sim has no timer hardware, so
 * the test sets INTCON<TMR0IF> and fires the interrupt; the ISR clears
 * the flag and invokes the callback.
 *
 * SFR addresses and bit masks are from the PIC16F877A data sheet
 * (DS39582B) via epic-hal's pic16f87xa_sfr.h.
 */

#ifndef HAL_PIC16_H
#define HAL_PIC16_H

#include <stdint.h>
#include <stdbool.h>

#define EPIC_BIT(n) (1U << (n))
#define EPIC_BIT_SET(reg, mask) ((reg) |= (uint8_t)(mask))
#define EPIC_BIT_CLR(reg, mask) ((reg) &= (uint8_t)~(mask))

#define EPIC_REG8(addr) (*(volatile uint8_t *)(uintptr_t)(addr))
#define EPIC_SFR(addr)  (*(volatile uint8_t *)(uintptr_t)(addr))

/* PIC16F877A SFR addresses (DS39582B). */
#define PIC_REG_TMR0    0x01U
#define PIC_REG_PORTB   0x06U
#define PIC_REG_TRISB   0x86U
#define PIC_REG_OPTION  0x81U
#define PIC_REG_INTCON  0x0BU
#define PIC_REG_PIR1    0x0CU
#define PIC_REG_PIR2    0x0DU
#define PIC_REG_PIE1    0x8CU
#define PIC_REG_PIE2    0x8DU

/* INTCON bits (DS39582B Register 4-1). */
#define PIC_INTCON_RBIF   EPIC_BIT(0)
#define PIC_INTCON_RBIE   EPIC_BIT(3)
#define PIC_INTCON_TMR0IF EPIC_BIT(2)
#define PIC_INTCON_TMR0IE EPIC_BIT(5)
#define PIC_INTCON_PEIE   EPIC_BIT(6)
#define PIC_INTCON_GIE    EPIC_BIT(7)

#define PIC_OPTION_T0CS  EPIC_BIT(5)
#define PIC_OPTION_PSA   EPIC_BIT(3)
#define PIC_OPTION_T0SE  EPIC_BIT(4)
#define PIC_OPTION_PS_MASK 0x07U

/* Status codes (epic-common core/hal_status.h). */
typedef enum {
    EPIC_OK      = 0x00U,
    EPIC_ERROR   = 0x01U,
    EPIC_BUSY    = 0x02U,
    EPIC_TIMEOUT = 0x03U,
    EPIC_INVALID = 0x04U
} EPIC_StatusTypeDef;

#endif /* HAL_PIC16_H */
