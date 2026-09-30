/* MSSP driver, 87XA flavor. Source: DS39582B section 9.0. Thin shim:
 * the shared body (hal/pic14/core/include/peripherals/pic14_ssp.h)
 * carries the driver; this header picks the family umbrella + SFR map.
 * The 87XA has no SSPMSK, so PIC14MIDRANGE_HAS_SSPMSK (in
 * pic14_midrange.h) selects the 87XA enum flavor. */

#ifndef PIC16F87XA_SSP_H
#define PIC16F87XA_SSP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"
#include "peripherals/pic14_ssp.h"

#endif /* PIC16F87XA_SSP_H */
