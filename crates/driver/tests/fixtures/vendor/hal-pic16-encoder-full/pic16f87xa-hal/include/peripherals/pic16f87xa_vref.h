/* Comparator Voltage Reference driver. Source: DS39582B §13.0,
 * Register 13-1. 16-tap resistor ladder: CVRR=0 gives 0..0.75 VDD in
 * VDD/24 steps; CVRR=1 gives 0.25..0.75 VDD in VDD/32 steps. CVROE
 * routes the output to RA2/AN2/VREF-, shared with the comparator/ADC
 * reference. */

#ifndef PIC16F87XA_VREF_H
#define PIC16F87XA_VREF_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief Voltage-reference range select (CVRCON<CVRR>, Register 13-1).
 */
typedef enum {
    VREF_RANGE_LOW  = 0x0U,   /* 0..0.75 VDD  (steps of VDD/24) */
    VREF_RANGE_HIGH = 0x1U,   /* 0.25..0.75 VDD (steps of VDD/32) */
} VREF_RangeTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    VREF_RangeTypeDef      Range;     /* Low or high range. */
    uint8_t                Value;     /* 0..15, ladder tap. */
    bool                   OutputEnable;  /* Route to RA2. */
    bool                   Enabled;        /* CVREN. */
} VREF_HandleTypeDef;

#define VREF_HANDLE_DEFAULT {                                              \
    .Range         = VREF_RANGE_LOW,                                       \
    .Value         = 0,                                                    \
    .OutputEnable  = false,                                                \
    .Enabled       = false,                                                \
}

/**
 * @brief  Initialize the voltage reference with the given handle.
 *         Programs CVRCON (range, tap, output enable, enable).
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

#endif /* PIC16F87XA_VREF_H */
