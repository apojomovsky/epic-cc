/* BOR / POR status helpers, shared by both builds. The build-specific
 * EPIC_WDT_Refresh / EPIC_Sleep_Enter live in the _sim / _target twins,
 * selected at link time; the helpers here only read and clear PCON
 * bits through the platform SFR macro. */

#include "core/pic16f87xa_wdt_sleep.h"

/* PCON bits (DS39582B §14.10, Register 14-2). */
#define PIC_PCON_BOR   EPIC_BIT(0)
#define PIC_PCON_POR   EPIC_BIT(1)

/* PCON lives in Bank 1 (0x8E). Plain EPIC_REG8 accesses silently
 * misdirect to the Bank-0 alias (TMR1L) under XC8 v4.00 (same class as
 * the TXSTA/OPTION_REG sites in tests/sim_bank_probe.c), so reads and
 * RMWs go through the safe Bank-1 macros where they exist. */

/**
 * @brief Return whether the last reset was a Brown-out Reset (PCON<BOR>).
 * @return 1 if BOR was the reset cause, 0 otherwise.
 */
uint8_t EPIC_BOR_GetStatus(void)
{
#ifdef EPIC_BANK1_READ8
    uint8_t pcon = 0u;
    EPIC_BANK1_READ8(PCON, pcon);
    return (pcon & PIC_PCON_BOR) ? 1U : 0U;
#else
    return (EPIC_REG8(PIC_REG_PCON) & PIC_PCON_BOR) ? 1U : 0U;
#endif
}

/**
 * @brief Clear PCON<BOR>.
 */
void EPIC_BOR_ClearFlag(void)
{
#ifdef EPIC_BANK1_READ8
    uint8_t pcon = 0u;
    EPIC_BANK1_READ8(PCON, pcon);
    pcon &= (uint8_t)~PIC_PCON_BOR;
    EPIC_BANK1_WRITE8(PCON, pcon);
#else
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PCON), PIC_PCON_BOR);
#endif
}

/**
 * @brief Return whether the device powered on via a Power-on Reset
 *        (PCON<POR>).
 * @return 1 if POR was the reset cause, 0 otherwise.
 */
uint8_t EPIC_POR_GetStatus(void)
{
#ifdef EPIC_BANK1_READ8
    uint8_t pcon = 0u;
    EPIC_BANK1_READ8(PCON, pcon);
    return (pcon & PIC_PCON_POR) ? 1U : 0U;
#else
    return (EPIC_REG8(PIC_REG_PCON) & PIC_PCON_POR) ? 1U : 0U;
#endif
}

/**
 * @brief Clear PCON<POR>.
 */
void EPIC_POR_ClearFlag(void)
{
#ifdef EPIC_BANK1_READ8
    uint8_t pcon = 0u;
    EPIC_BANK1_READ8(PCON, pcon);
    pcon &= (uint8_t)~PIC_PCON_POR;
    EPIC_BANK1_WRITE8(PCON, pcon);
#else
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PCON), PIC_PCON_POR);
#endif
}
