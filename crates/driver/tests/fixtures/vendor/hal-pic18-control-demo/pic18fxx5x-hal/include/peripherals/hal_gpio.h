/*
 * Family-neutral GPIO contract (`EPIC_GPIO_*` + RB-change hook). Each
 * family ships its own copy; the build's include path picks which one
 * resolves. Also carries the RB<7:4> change-interrupt hook `epic-encoder`
 * builds on.
 */

#ifndef EPIC_GPIO_H
#define EPIC_GPIO_H
#include "pic18fxx5x_gpio.h"
#endif /* EPIC_GPIO_H */
