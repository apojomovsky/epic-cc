/* Shared PIC14 mid-range IRQ table contract: the per-source
 * flag/enable descriptor, the PIR address selector and the table
 * itself. Each family defines the table against its own IRQn enum in
 * src/core/pic16_irq_table.c; the shared pic14_irq.c body consumes it
 * through this header. pir_reg_addr stays a macro: a function-call
 * boundary here lost the returned address before it reached the
 * caller's read/write (XC8 codegen gotcha, see the family README).
 * PIR1 = 0x0C, PIR2 = 0x0D (single-PIR parts have PIR1 only). */

#ifndef PIC14_IRQ_COMMON_H
#define PIC14_IRQ_COMMON_H

#include "pic14_midrange.h"

/* Per-IRQ descriptor. Flags and enables sit in INTCON, PIR1/PIR2
 * (Bank 0), and PIE1/PIE2 (Bank 1); Bank-1 mirrors of PIR1/PIR2 are at
 * 0x8C/0x8D. Flag and enable residency is tracked separately: on the
 * 83/84/84A the EEPROM enable (EEIE) is INTCON<6> while its flag
 * (EEIF) is EECON1<4> (DS35007B §3.0/§14.11). */
typedef struct {
    uint8_t flag_mask;        /**< PIR/EECON1/INTCON bit to test/clear. */
    uint8_t enable_mask;      /**< PIE/INTCON bit to set/clear. */
    uint8_t flag_in_intcon;   /**< 1 = flag lives in INTCON. */
    uint8_t enable_in_intcon; /**< 1 = enable lives in INTCON. */
    uint8_t pir_is_pir2;      /**< 1 = PIR2, 0 = PIR1. (Unused when the
                                   matching flag/enable is INTCON.) */
} irq_desc_t;

#if PIC14MIDRANGE_HAS_PIR2
#define pir_reg_addr(d) ((d)->pir_is_pir2 ? PIC_REG_PIR2 : PIC_REG_PIR1)
#elif PIC14MIDRANGE_HAS_PIR1
#define pir_reg_addr(d) (PIC_REG_PIR1)
#else
/* No PIR registers on this family (83/84/84A): the only non-INTCON
 * flag is the EEPROM completion bit, EECON1<4>, and its accesses go
 * through the family platform's EPIC_BANK1_* macros (Bank-1 RMW under
 * XC8), not through a plain PIR address. The PIR accessors below
 * compile out together with their callsites; do not name PIC_REG_PIR1
 * at all. */
#endif

extern const irq_desc_t irq_table[];
/* Table length for the shared bound check. A macro, not a const
 * global: the pinned epic-cc isel turns every const global into a
 * flash table and panics on scalar ones (no table bytes), so a
 * `const unsigned` here breaks the epiccc gate. Each family's own
 * core/pic16_irq.h defines IRQ_TABLE_SIZE next to its IRQn enum. */

#endif /* PIC14_IRQ_COMMON_H */
