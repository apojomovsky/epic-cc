/* Comparator driver, two on-chip comparators. Source: DS39582B §12.0,
 * Register 12-1, Figure 12-1; full reference: MANUAL.md §17. Inputs
 * multiplex onto PORTA (RA0-RA3, RA5, or VREF); outputs on RA4/RA5
 * when enabled. The 8 operating modes are on COMP_ModeTypeDef below. */

#ifndef PIC16F87XA_COMP_H
#define PIC16F87XA_COMP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief Comparator operating mode (CMCON<CM2:CM0>, Figure 12-1).
 */
typedef enum {
    COMP_MODE_RESET          = 0x0U,   /* 000, comparators in reset. */
    COMP_MODE_ONE_WITH_OUT   = 0x1U,   /* 001, C1 only, output on RA4. */
    COMP_MODE_TWO_INDEP      = 0x2U,   /* 010, C1 and C2, no outputs. */
    COMP_MODE_TWO_WITH_OUT   = 0x3U,   /* 011, C1 and C2, outputs on RA4/RA5. */
    COMP_MODE_TWO_COMMON     = 0x4U,   /* 100, common ref, no outputs. */
    COMP_MODE_TWO_COMMON_OUT = 0x5U,   /* 101, common ref, with outputs. */
    COMP_MODE_FOUR_MUXED     = 0x6U,   /* 110, 4 inputs muxed to 2 comparators. */
    COMP_MODE_OFF            = 0x7U,   /* 111, comparators off. */
} COMP_ModeTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    COMP_ModeTypeDef      Mode;
    bool                  C1Inverted;     /* C1INV bit. */
    bool                  C2Inverted;     /* C2INV bit. */
    bool                  CIS;            /* Comparator input switch. */
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
 * @brief  Initialize the comparator driver with the given handle.
 *         Programs CMCON (mode, inversions, input switch) and installs
 *         the change callback.
 * @param h handle with Mode, C1Inverted, C2Inverted, CIS, ChangeCallback.
 * @return EPIC_OK on success, EPIC_ERROR if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_COMP_Init(const COMP_HandleTypeDef *h);

/**
 * @brief  De-initialize the comparator driver. Returns CMCON to reset
 *         and clears the change callback.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_COMP_DeInit(void);

/**
 * @brief Returns 1 if C1 output is high (CMCON<C1OUT>).
 * @return 1 if C1 output is high, 0 otherwise.
 */
uint8_t EPIC_COMP_C1Out(void);

/**
 * @brief Returns 1 if C2 output is high (CMCON<C2OUT>).
 * @return 1 if C2 output is high, 0 otherwise.
 */
uint8_t EPIC_COMP_C2Out(void);

/**
 * @brief Returns 1 if CMIF is set.
 * @return 1 if the comparator change flag is set, 0 otherwise.
 */
uint8_t EPIC_COMP_IsChangeFlag(void);

/**
 * @brief Clear the CMIF flag (must be done in the change IRQ).
 */
void EPIC_COMP_ClearChangeFlag(void);

/**
 * @brief Weak comparator ISR, override in user code.
 */
void COMP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_COMP_H */
