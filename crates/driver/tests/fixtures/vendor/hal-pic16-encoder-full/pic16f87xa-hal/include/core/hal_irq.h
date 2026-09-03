/* Family-neutral EPIC_IRQ_* contract. Family-agnostic consumers include
 * this name instead of pic16_irq.h; each family provides its own copy
 * that pulls in its family-specific irq header. The include path
 * selects which family's copy resolves. */

#ifndef EPIC_HAL_IRQ_H
#define EPIC_HAL_IRQ_H
#include "pic16_irq.h"
#endif /* EPIC_HAL_IRQ_H */
