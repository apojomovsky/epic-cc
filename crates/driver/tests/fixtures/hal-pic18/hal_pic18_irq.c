/* Vendored slice of the real epic-hal pic18fxx5x-hal IRQ controller
 * (DS39632E §9.0), adapted to compile under epic-cc. The smoke only needs
 * the global master enable (INTCON GIE); the Timer0/Timer2 source enables
 * and flags are written inline by the vendored timer drivers (the real
 * HAL's EPIC_IRQ_Enable/ClearFlag/GetFlag walk an IRQ descriptor table
 * and dispatch through an LLVM `switch`, which irparse does not lower).
 * Single-vector compatibility mode (IPEN clear, ADR-013).
 */

#include "hal_pic18.h"

void EPIC_IRQ_Restore(uint8_t prev_state)
{
    if (prev_state != 0u) {
        uint8_t v = EPIC_REG8(PIC_REG_INTCON);
        EPIC_REG8(PIC_REG_INTCON) = (uint8_t)(v | PIC_INTCON_GIE);
    }
}
