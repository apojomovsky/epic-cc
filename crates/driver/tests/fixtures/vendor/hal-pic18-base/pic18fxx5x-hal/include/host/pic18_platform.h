/*
 * Host-simulation platform half of the SFR mapping layer (target half:
 * `target/pic18_platform.h`); the build's include path picks one, so
 * `pic18fxx5x.h` includes `"pic18_platform.h"` unconditionally. Every
 * SFR access indexes a memory-backed register file `pic18_sim_sfr[]`
 * (`src/sim/pic18_sim.c`), sized to the full 12-bit data-memory address
 * space so tests can poke any register directly.
 */

#ifndef PIC18_PLATFORM_H
#define PIC18_PLATFORM_H

#include <stdint.h>

/* 4096-byte memory-backed register file (DS39632E Figure 5-5 data-memory
 * map footprint), defined in src/sim/pic18_sim.c. */
extern uint8_t pic18_sim_sfr[0x1000];

/* GCC/Clang weak attribute, lets user code override a peripheral's
 * IRQHandler if it ever needs to. */
#define EPIC_WEAK   __attribute__((weak))

/* Placement pins are an XC8 extension; the host build is a no-op. */
#define EPIC_PLACE(addr)

/* SFR access resolves to an index into the simulated register file. */
#define EPIC_SFR_PTR(addr)       (&pic18_sim_sfr[(uint16_t)(addr)])
#define epic_sfr_read8(addr)     (pic18_sim_sfr[(uint16_t)(addr)])
#define epic_sfr_write8(addr, v) \
    do { pic18_sim_sfr[(uint16_t)(addr)] = (uint8_t)(v); } while (0)

/* Address of a register as a uint8_t lvalue (read/write/RMW). */
#define EPIC_REG8(addr)          (pic18_sim_sfr[(uint16_t)(addr)])

#endif /* PIC18_PLATFORM_H */
