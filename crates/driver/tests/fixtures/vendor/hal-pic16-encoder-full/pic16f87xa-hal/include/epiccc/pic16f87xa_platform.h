/* epic-cc variant of the SFR mapping layer (paired with
 * target/pic16f87xa_platform.h and host/pic16f87xa_platform.h); the build's
 * include path picks which resolves, so pic16f87xa.h includes
 * "pic16f87xa_platform.h" unconditionally with no #ifdef.
 *
 * Banking is inserted by the compiler's banking pass, so the XC8-specific
 * inline-asm pie/bank fixups are plain C here. Placement pins are dropped
 * (EPIC_PLACE expands to nothing): the overlay places globals itself. */

#ifndef PIC16F87XA_PLATFORM_H
#define PIC16F87XA_PLATFORM_H

#include <stdint.h>

/* Prefer the toolchain header when building with epic-cc; fall back to a
 * local definition for host-gcc smoke checks. */
#ifdef __has_include
#if __has_include(<epic-cc.h>)
#include <epic-cc.h>
#else
#define EPIC_AT(addr) __attribute__((section(".epicat." #addr)))
#define EPIC_CONFIG(spec) \
    static const char __epic_config[] __attribute__((used, section(".epiccfg." spec))) = spec
#ifndef EPIC_FOSC_HZ
#define EPIC_FOSC_HZ 0
#endif
#endif
#else
#include <epic-cc.h>
#endif

/* Weak attribute: lets user code override a peripheral's IRQHandler. */
#define EPIC_WEAK   __attribute__((weak))

/* Placement pins are an XC8 bank-placement concern; the
 * whole-program overlay places globals (unique addresses, one
 * bank) itself, so a pin only fragments the layout (epic-hal#86). */
#define EPIC_PLACE(addr)

/* Bridge the historic FOSC_HZ name to the epic-cc spelling so shared
 * harness code (epic_harness_target.c) sees the right frequency without
 * its own #ifdef. The driver pre-defines EPIC_FOSC_HZ from the resolved
 * EPIC_CONFIG. */
#ifndef FOSC_HZ
#ifdef EPIC_FOSC_HZ
#define FOSC_HZ EPIC_FOSC_HZ
#endif
#endif

/* SFR access resolves to a direct volatile dereference of the address. */
#define EPIC_SFR_PTR(addr)       ((volatile uint8_t *)(uintptr_t)(addr))
#define epic_sfr_read8(addr)     (*(volatile uint8_t *)(uintptr_t)(addr))
#define epic_sfr_write8(addr, v) \
    do { *(volatile uint8_t *)(uintptr_t)(addr) = (uint8_t)(v); } while (0)

/* Address of a register as a uint8_t lvalue (read/write/RMW). */
#define EPIC_REG8(addr)          (*(volatile uint8_t *)(uintptr_t)(addr))

/* Legacy bare name OPTION_REG (XC8 SFR symbol) maps to its address. The
 * peripherals pass OPTION_REG to EPIC_BANK1_*; token pasting then needs
 * PIC_REG_OPTION_REG to exist. */
#define PIC_REG_OPTION_REG PIC_REG_OPTION

/* Alias for the legacy bare name used in BANK macro call sites. */
#ifndef OPTION_REG
#define OPTION_REG PIC_REG_OPTION
#endif

/* Banked SFR access under epic-cc is a plain C access; the compiler's
 * banking pass inserts the required BANKSEL. No inline asm. */
#define EPIC_BANK1_WRITE8(sfr_name, value) \
    do { EPIC_REG8(PIC_REG_##sfr_name) = (uint8_t)(value); } while (0)
#define EPIC_BANK1_READ8(sfr_name, out_var) \
    do { (out_var) = EPIC_REG8(PIC_REG_##sfr_name); } while (0)
#define EPIC_BANK2_WRITE8(sfr, val)  do { EPIC_REG8(PIC_REG_##sfr) = (uint8_t)(val); } while (0)
#define EPIC_BANK2_READ8(sfr, out)   do { (out) = EPIC_REG8(PIC_REG_##sfr); } while (0)
#define EPIC_BANK3_WRITE8(sfr, val)  do { EPIC_REG8(PIC_REG_##sfr) = (uint8_t)(val); } while (0)
#define EPIC_BANK3_READ8(sfr, out)   do { (out) = EPIC_REG8(PIC_REG_##sfr); } while (0)

/* PIE1 (0x8C) / PIE2 (0x8D) enable/disable, plain RMW. */
#define EPIC_PIE_ENABLE_BIT(is_pir2, mask) \
    do { \
        if (is_pir2) { EPIC_REG8(0x8DU) |= (uint8_t)(mask); } \
        else         { EPIC_REG8(0x8CU) |= (uint8_t)(mask); } \
    } while (0)

#define EPIC_PIE_DISABLE_BIT(is_pir2, mask) \
    do { \
        if (is_pir2) { EPIC_REG8(0x8DU) &= (uint8_t)~(mask); } \
        else         { EPIC_REG8(0x8CU) &= (uint8_t)~(mask); } \
    } while (0)

/* PIE1/PIE2 enable-bit reads for the dispatch tiers: TMR1IE, TXIE,
 * TMR2IE, SSPIE, ADIE, CCP1IE, CCP2IE, EEIE. */
#define EPIC_PIE1_READ_TMR1IE(out_var) ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE1_READ_TXIE(out_var)   ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE1_READ_TMR2IE(out_var) ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE1_READ_SSPIE(out_var)  ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE1_READ_ADIE(out_var)   ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE1_READ_CCP1IE(out_var) ((out_var) = EPIC_REG8(0x8CU))
#define EPIC_PIE2_READ_CCP2IE(out_var) ((out_var) = EPIC_REG8(0x8DU))
#define EPIC_PIE2_READ_EEIE(out_var)   ((out_var) = EPIC_REG8(0x8DU))

#endif /* PIC16F87XA_PLATFORM_H */
