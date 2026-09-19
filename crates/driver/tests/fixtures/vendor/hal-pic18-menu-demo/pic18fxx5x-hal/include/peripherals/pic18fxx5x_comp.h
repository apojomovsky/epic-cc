/*
 * Two on-chip comparators (DS39632E §22.0), 8 modes selected by
 * CMCON<CM2:CM0> (Figure 22-1). Same bit layout as the PIC16 comparator,
 * in the Access Bank (0xFB4, no bank switching); the comparator voltage
 * reference (CVRCON) is a separate module.
 */

#ifndef PIC18FXX5X_COMP_H
#define PIC18FXX5X_COMP_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief Comparator operating mode (CMCON<CM2:CM0>, DS39632E Figure 22-1).
 *        Same eight modes as the PIC16 comparator.
 */
typedef enum {
    COMP_MODE_RESET          = 0x0U,   /**< 000, comparators in reset. */
    COMP_MODE_ONE_WITH_OUT   = 0x1U,   /**< 001, C1 only, output on RA4. */
    COMP_MODE_TWO_INDEP      = 0x2U,   /**< 010, C1 and C2, no outputs. */
    COMP_MODE_TWO_WITH_OUT   = 0x3U,   /**< 011, C1 and C2, outputs on RA4/RA5. */
    COMP_MODE_TWO_COMMON     = 0x4U,   /**< 100, common ref, no outputs. */
    COMP_MODE_TWO_COMMON_OUT = 0x5U,   /**< 101, common ref, with outputs. */
    COMP_MODE_FOUR_MUXED     = 0x6U,   /**< 110, 4 inputs muxed to 2 comparators. */
    COMP_MODE_OFF            = 0x7U,   /**< 111, comparators off (POR default). */
} COMP_ModeTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    COMP_ModeTypeDef      Mode;
    bool                   C1Inverted;     /**< C1INV bit. */
    bool                   C2Inverted;     /**< C2INV bit. */
    bool                   CIS;            /**< Comparator input switch. */
    /** @brief Optional change callback (fires on CMIF). */
    void (*ChangeCallback)(void);
} COMP_HandleTypeDef;

#define COMP_HANDLE_DEFAULT {                                              \
    .Mode            = COMP_MODE_TWO_INDEP,                                \
    .C1Inverted      = false,                                              \
    .C2Inverted      = false,                                              \
    .CIS             = false,                                              \
    .ChangeCallback  = NULL,                                               \
}

/**
 * @brief  Configure the comparators: mode, inversion, input switch and
 *         change callback, then enable the module (CMCON).
 * @param h the comparator handle describing the desired configuration.
 * @return 0 on success, 0xFFFF on invalid configuration.
 */
EPIC_StatusTypeDef EPIC_COMP_Init(const COMP_HandleTypeDef *h);

/**
 * @brief  Disable the comparators (CMCON = COMP_MODE_OFF) and clear CMIF.
 * @return 0 on success, 0xFFFF if the module was not initialized.
 */
EPIC_StatusTypeDef EPIC_COMP_DeInit(void);

/**
 * @brief Returns 1 if C1 output is high (CMCON<C1OUT>).
 * @return 1 when comparator 1 output is high, else 0.
 */
uint8_t EPIC_COMP_C1Out(void);

/**
 * @brief Returns 1 if C2 output is high (CMCON<C2OUT>).
 * @return 1 when comparator 2 output is high, else 0.
 */
uint8_t EPIC_COMP_C2Out(void);

/**
 * @brief Returns 1 if CMIF is set.
 * @return 1 when the comparator-change flag is set, else 0.
 */
uint8_t EPIC_COMP_IsChangeFlag(void);

/**
 * @brief Clear the CMIF flag (must be done in the change IRQ).
 */
void EPIC_COMP_ClearChangeFlag(void);

/**
 * @brief Comparator change interrupt handler (weak default).
 */
void COMP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_COMP_H */
