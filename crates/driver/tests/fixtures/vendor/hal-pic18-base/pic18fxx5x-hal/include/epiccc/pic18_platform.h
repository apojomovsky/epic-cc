/* epic-cc variant of the SFR mapping layer (paired with
 * target/pic18_platform.h and host/pic18_platform.h); the build's include
 * path picks which resolves, so pic18fxx5x.h includes "pic18_platform.h"
 * unconditionally with no #ifdef.
 *
 * Under epic-cc the SFR layer is the same volatile dereference shape as
 * the XC8 target; PIC18 has an Access Bank and no BSR, so no banking
 * fixups are needed. Placement uses EPIC_AT from <epic-cc.h>. */

#ifndef PIC18_PLATFORM_H
#define PIC18_PLATFORM_H

#include <stdint.h>

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

#define EPIC_WEAK   __attribute__((weak))
#define EPIC_PLACE(addr) EPIC_AT(addr)

/* Bridge the historic FOSC_HZ name to the epic-cc spelling so shared
 * harness code sees the right frequency without its own #ifdef. */
#ifndef FOSC_HZ
#ifdef EPIC_FOSC_HZ
#define FOSC_HZ EPIC_FOSC_HZ
#endif
#endif

#define EPIC_SFR_PTR(addr)       ((volatile uint8_t *)(uintptr_t)(addr))
#define epic_sfr_read8(addr)     (*(volatile uint8_t *)(uintptr_t)(addr))
#define epic_sfr_write8(addr, v) \
    do { *(volatile uint8_t *)(uintptr_t)(addr) = (uint8_t)(v); } while (0)

#define EPIC_REG8(addr)          (*(volatile uint8_t *)(uintptr_t)(addr))

#endif /* PIC18_PLATFORM_H */
