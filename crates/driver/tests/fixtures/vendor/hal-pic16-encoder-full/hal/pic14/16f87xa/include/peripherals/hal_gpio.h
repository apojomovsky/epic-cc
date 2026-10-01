/* Family-neutral GPIO contract (EPIC_GPIO_* + RB-change hook). Pulls
 * in the family-specific gpio header; the include path picks which
 * family's copy resolves. epic-encoder builds on the RB<7:4> change
 * hook carried here. */

#ifndef EPIC_GPIO_H
#define EPIC_GPIO_H
#include "pic16f87xa_gpio.h"
#endif /* EPIC_GPIO_H */
