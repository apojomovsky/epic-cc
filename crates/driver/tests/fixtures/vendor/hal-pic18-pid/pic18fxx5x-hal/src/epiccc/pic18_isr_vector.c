/* epic-cc interrupt-vector entry (single-vector compatibility mode).
 * PIC18 has two vectors at 0008h/0018h (DS39632E 9.0) but epic-cc's
 * backend supports only one (ADR-013); both XC8 handlers delegating
 * to epic_dispatch_all_irqs is correct because every peripheral
 * handler checks its own flag. */

/**
 * @brief Shared IRQ fan-out dispatcher.
 */
extern void epic_dispatch_all_irqs(void);

/**
 * @brief Single interrupt vector for the epic-cc build, delegates to the
 *        shared dispatcher. Replaces the XC8 high/low priority pair.
 */
void __attribute__((interrupt(0))) PIC18_IRQ_Handler(void)
{
    epic_dispatch_all_irqs();
}
