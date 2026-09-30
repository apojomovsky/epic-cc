/**
 * Family-blind part of the `EPIC_IRQ_*` contract: the shared priority
 * enum. PIC16 has one vector and ignores priority; PIC18 has two
 * (DS39632E §9.0) and routes by IPR/INTCON2/INTCON3 bits.
 */

#ifndef EPIC_IRQ_H
#define EPIC_IRQ_H

/**
 * Interrupt priority level. Shared vocabulary; the effect is
 * family-specific (PIC16 ignores it, PIC18 routes the source to the
 * high or low vector via its IPR bits).
 */
typedef enum {
    EPIC_IRQ_PRIORITY_LOW  = 0,  /**< Low-priority vector (PIC18 0018h). */
    EPIC_IRQ_PRIORITY_HIGH = 1   /**< High-priority vector (PIC18 0008h). */
} EPIC_IRQ_Priority;

#endif /* EPIC_IRQ_H */
