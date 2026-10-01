/* Shared PIC14 mid-range BOR/POR helpers (+ WDTCON software control
 * where present), both builds. The build-specific EPIC_WDT_Refresh /
 * EPIC_Sleep_Enter live in the _sim / _target twins, selected at link
 * time; the helpers here only read and clear PCON bits (and set
 * WDTCON) through the platform SFR macro. Sources: DS39582B §14.10
 * (87XA), DS40001291H §14.2 (88X). */

#include "core/pic14_wdt_sleep.h"

/* PCON bits (DS39582B §14.10, Register 14-2). */
#define PIC_PCON_BOR   EPIC_BIT(0)
#define PIC_PCON_POR   EPIC_BIT(1)

/* PCON lives in Bank 1 (0x8E). Plain EPIC_REG8 accesses silently
 * misdirect to the Bank-0 alias (TMR1L) under XC8 v4.00 (same class as
 * the TXSTA/OPTION_REG sites in tests/sim_bank_probe.c), so reads and
 * RMWs go through the safe Bank-1 macros where they exist. */

#if PIC14MIDRANGE_HAS_PCON
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
#endif /* PIC14MIDRANGE_HAS_PCON */

#if PIC14MIDRANGE_HAS_WDT_SW
/* WDTCON software control (Bank 2, 0x105 on 88X; Bank 1, 0x97 on the
 * 4-bank 63x/67x/68x parts; Bank 0, 0x18 on the 639-shape parts, plain
 * literal access). Plain EPIC_REG8 accesses silently misdirect to
 * the Bank-0 alias under XC8 v4.00, so reads and RMWs go through the
 * safe banked macros where they exist. */

/**
 * @brief Enable or disable the software Watchdog Timer (WDTCON<SWDTEN>).
 * @param enable 1 to turn the WDT on, 0 to turn it off.
 */
void EPIC_WDT_SetSoftwareEnable(uint8_t enable)
{
#if PIC14MIDRANGE_HAS_WDTCON_BANK1 && defined(EPIC_BANK1_READ8)
    uint8_t wdtcon = 0u;
    EPIC_BANK1_READ8(WDTCON, wdtcon);
    if (enable) wdtcon |= PIC_WDTCON_SWDTEN;
    else        wdtcon &= (uint8_t)~PIC_WDTCON_SWDTEN;
    EPIC_BANK1_WRITE8(WDTCON, wdtcon);
#elif PIC14MIDRANGE_HAS_WDTCON_BANK0
    /* Bank-0 WDTCON (63x 2-bank WDT shapes): plain access needs no
     * switch, and every caller runs at bank 0 by convention. The
     * address rides the WDTCON_BANK0 alias (the Bank-1 constant does
     * not exist there). */
    uint8_t wdtcon = EPIC_REG8(PIC_REG_WDTCON_BANK0);
    if (enable) wdtcon |= PIC_WDTCON_SWDTEN;
    else        wdtcon &= (uint8_t)~PIC_WDTCON_SWDTEN;
    EPIC_REG8(PIC_REG_WDTCON_BANK0) = wdtcon;
#elif !PIC14MIDRANGE_HAS_WDTCON_BANK0 && defined(EPIC_BANK2_READ8)
    uint8_t wdtcon = 0u;
    EPIC_BANK2_READ8(WDTCON, wdtcon);
    if (enable) wdtcon |= PIC_WDTCON_SWDTEN;
    else        wdtcon &= (uint8_t)~PIC_WDTCON_SWDTEN;
    EPIC_BANK2_WRITE8(WDTCON, wdtcon);
#else
    uint8_t wdtcon = EPIC_REG8(PIC_REG_WDTCON);
    if (enable) wdtcon |= PIC_WDTCON_SWDTEN;
    else        wdtcon &= (uint8_t)~PIC_WDTCON_SWDTEN;
    EPIC_REG8(PIC_REG_WDTCON) = wdtcon;
#endif
}

/**
 * @brief Set the WDT period prescaler (WDTCON<WDTPS3:WDTPS0>).
 * @param wdtps the 4-bit WDTPS value (0..11; 12..15 reserved).
 */
void EPIC_WDT_SetPrescaler(uint8_t wdtps)
{
#if PIC14MIDRANGE_HAS_WDTCON_BANK1 && defined(EPIC_BANK1_READ8)
    uint8_t wdtcon = 0u;
    EPIC_BANK1_READ8(WDTCON, wdtcon);
    wdtcon &= (uint8_t)~PIC_WDTCON_WDTPS_MASK;
    wdtcon |= (uint8_t)((wdtps << PIC_WDTCON_WDTPS_POS) & PIC_WDTCON_WDTPS_MASK);
    EPIC_BANK1_WRITE8(WDTCON, wdtcon);
#elif PIC14MIDRANGE_HAS_WDTCON_BANK0
    uint8_t wdtcon = EPIC_REG8(PIC_REG_WDTCON_BANK0);
    wdtcon &= (uint8_t)~PIC_WDTCON_WDTPS_MASK;
    wdtcon |= (uint8_t)((wdtps << PIC_WDTCON_WDTPS_POS) & PIC_WDTCON_WDTPS_MASK);
    EPIC_REG8(PIC_REG_WDTCON_BANK0) = wdtcon;
#elif !PIC14MIDRANGE_HAS_WDTCON_BANK0 && defined(EPIC_BANK2_READ8)
    uint8_t wdtcon = 0u;
    EPIC_BANK2_READ8(WDTCON, wdtcon);
    wdtcon &= (uint8_t)~PIC_WDTCON_WDTPS_MASK;
    wdtcon |= (uint8_t)((wdtps << PIC_WDTCON_WDTPS_POS) & PIC_WDTCON_WDTPS_MASK);
    EPIC_BANK2_WRITE8(WDTCON, wdtcon);
#else
    uint8_t wdtcon = EPIC_REG8(PIC_REG_WDTCON);
    wdtcon &= (uint8_t)~PIC_WDTCON_WDTPS_MASK;
    wdtcon |= (uint8_t)((wdtps << PIC_WDTCON_WDTPS_POS) & PIC_WDTCON_WDTPS_MASK);
    EPIC_REG8(PIC_REG_WDTCON) = wdtcon;
#endif
}
#endif
