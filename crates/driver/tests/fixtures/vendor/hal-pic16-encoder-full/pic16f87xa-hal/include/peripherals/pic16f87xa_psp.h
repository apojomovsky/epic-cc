/* Parallel Slave Port driver, 40/44-pin only (PIC16F873A/876A have no
 * PSP); gated by PIC16F87XA_FAMILY_HAS_PSP. Source: DS39582B §4.5;
 * full reference: MANUAL.md §20. Manages PSPMODE, IBF/OBF/IBOV and the
 * PSP interrupt; an external master drives CS/RD/WR on real silicon,
 * the sim backend via pic16f87xa_sim.h. */

#ifndef PIC16F87XA_PSP_H
#define PIC16F87XA_PSP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

#if !PIC16F87XA_FAMILY_HAS_PSP
#error "pic16f87xa_psp.h is for 40/44-pin PIC16F87XA parts only"
#endif

/**
 * @brief  Initialize the Parallel Slave Port. Configures the PSP pins,
 *         programs TRISE<PSPMODE> and installs the data callback.
 * @param callback optional function called when the master writes a
 *        byte (fires on PSPIF), or NULL for polling.
 * @return EPIC_OK on success, EPIC_ERROR if `callback` is required.
 */
EPIC_StatusTypeDef EPIC_PSP_Init(void (*callback)(void));

/**
 * @brief  De-initialize the Parallel Slave Port. Clears PSPMODE, the
 *         interrupt enable and the callback.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_PSP_DeInit(void);

/**
 * @brief Enable Parallel Slave Port mode (TRISE<PSPMODE> = 1).
 */
void EPIC_PSP_Enable(void);

/**
 * @brief Disable Parallel Slave Port mode.
 */
void EPIC_PSP_Disable(void);

/**
 * @brief Returns 1 if the PSP input buffer is full (TRISE<IBF>).
 * @return 1 if the input buffer holds an unread byte, 0 otherwise.
 */
uint8_t EPIC_PSP_IsInputBufferFull(void);

/**
 * @brief Returns 1 if the PSP output buffer is full (TRISE<OBF>).
 * @return 1 if the output buffer is full, 0 otherwise.
 */
uint8_t EPIC_PSP_IsOutputBufferFull(void);

/**
 * @brief Returns 1 if an input buffer overflow occurred (TRISE<IBOV>).
 * @return 1 if an input overflow is flagged, 0 otherwise.
 */
uint8_t EPIC_PSP_HasInputOverflow(void);

/**
 * @brief Clear the IBOV flag. Must be done in software.
 */
void EPIC_PSP_ClearInputOverflow(void);

/**
 * @brief Weak PSP ISR, override in user code.
 */
void PSP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_PSP_H */
