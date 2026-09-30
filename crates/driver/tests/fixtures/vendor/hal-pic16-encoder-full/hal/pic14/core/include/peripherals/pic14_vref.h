/* Shared PIC14 mid-range voltage-reference driver (87XA CVRCON parts
 * and 88X/628A VRCON parts). Sources: DS39582B §13 (87XA),
 * DS40001291H §8.10 (88X); full reference: the family MANUAL.md.
 * 16-tap resistor ladder in two ranges; VROE routes the output to RA2.
 * The range bit polarity is genuinely per-generation (CVRR=1 is the
 * high range on 87XA, VRR=1 is the low range on VRCON parts, per each
 * datasheet's own register table), so each form programs its own
 * polarity; the millivolt math is range-name-keyed and shared. */

#ifndef PIC14_VREF_H
#define PIC14_VREF_H

#include "pic14_midrange.h"
#include "pic14_midrange_sfr.h"

#if PIC14MIDRANGE_HAS_VRCON
/**
 * @brief Voltage-reference range select (VRCON<VRR>, Register 8-5).
 */
typedef enum {
    VREF_RANGE_HIGH = 0x0U,   /* VDD/4 + (n/32)*VDD, 0.25..1.0 VDD. */
    VREF_RANGE_LOW  = 0x1U,   /* (n/24)*VDD, 0..0.625 VDD. */
} VREF_RangeTypeDef;

/**
 * @brief Reference source select (VRCON<VRSS>, Register 8-5).
 */
typedef enum {
    VREF_SRC_VDD_VSS   = 0x0U,   /**< CVRSRC = VDD - VSS. */
    VREF_SRC_VREFP_VREFN = 0x1U, /**< CVRSRC = VREF+ - VREF- pins. */
} VREF_SourceTypeDef;
#else
/**
 * @brief Voltage-reference range select (CVRCON<CVRR>, Register 13-1).
 */
typedef enum {
    VREF_RANGE_LOW  = 0x0U,   /* 0..0.75 VDD  (steps of VDD/24) */
    VREF_RANGE_HIGH = 0x1U,   /* 0.25..0.75 VDD (steps of VDD/32) */
} VREF_RangeTypeDef;
#endif

/** Driver handle (Cube-style). */
typedef struct {
    VREF_RangeTypeDef      Range;     /* Low or high range. */
#if PIC14MIDRANGE_HAS_VRSS
    VREF_SourceTypeDef     Source;    /* VDD-VSS or VREF pins (VRSS). */
#endif
    uint8_t                Value;     /* 0..15, ladder tap. */
    bool                   OutputEnable;  /* Route to RA2. */
    bool                   Enabled;        /* VREN. */
} VREF_HandleTypeDef;

#if PIC14MIDRANGE_HAS_VRSS
#define VREF_HANDLE_DEFAULT {                                              \
    .Range         = VREF_RANGE_HIGH,                                      \
    .Source        = VREF_SRC_VDD_VSS,                                     \
    .Value         = 0,                                                    \
    .OutputEnable  = false,                                                \
    .Enabled       = false,                                                \
}
#else
#define VREF_HANDLE_DEFAULT {                                              \
    .Range         = VREF_RANGE_LOW,                                       \
    .Value         = 0,                                                    \
    .OutputEnable  = false,                                                \
    .Enabled       = false,                                                \
}
#endif

/**
 * @brief  Initialize the voltage reference with the given handle.
 *         Programs the reference register (range, tap, output enable,
 *         enable, plus source where present).
 * @param h handle with Range, Value, OutputEnable, Enabled.
 * @return EPIC_OK on success, EPIC_ERROR if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_VREF_Init(const VREF_HandleTypeDef *h);

/**
 * @brief  De-initialize the voltage reference. Disables it and clears
 *         the output enable.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_VREF_DeInit(void);

/**
 * @brief  Compute the nominal output voltage (mV) for a given range +
 *         tap value. Assumes CVRSRC = Vdd_mv.
 * @param vdd_mv the supply voltage in millivolts.
 * @param range VREF_RANGE_LOW or VREF_RANGE_HIGH.
 * @param value the ladder tap, 0..15.
 * @return the nominal output voltage in millivolts.
 */
uint32_t EPIC_VREF_MilliVolts(uint32_t vdd_mv,
                             VREF_RangeTypeDef range,
                             uint8_t value);

#endif /* PIC14_VREF_H */
