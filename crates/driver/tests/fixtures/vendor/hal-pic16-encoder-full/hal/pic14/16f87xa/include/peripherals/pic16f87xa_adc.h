/* A/D converter driver, 87XA flavor. Source: DS39582B section 11.0.
 * Thin shim: the shared body (hal/pic14/core/include/
 * peripherals/pic14_adc.h) carries the driver; this header picks the
 * family umbrella + SFR map. PIC14MIDRANGE_HAS_ADC_PCFG (in
 * pic14_midrange.h) selects the 87XA reference/mux/clock flavors. */

#ifndef PIC16F87XA_ADC_H
#define PIC16F87XA_ADC_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"
#include "peripherals/pic14_adc.h"

#endif /* PIC16F87XA_ADC_H */
