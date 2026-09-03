/* Data EEPROM driver. Source: DS39582B §3.0; full register reference:
 * MANUAL.md §19. The driver hides the mandatory 0x55/0xAA unlock
 * sequence; writes are non-blocking, poll EEIF (PIR2<4>) or use the
 * callback for completion. */

#ifndef PIC16F87XA_EEPROM_H
#define PIC16F87XA_EEPROM_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief  Initialize the EEPROM driver. Programs PIE2<EEIE> if
 *         `callback` is non-NULL.
 * @param callback optional write-complete callback (fires on EEIF),
 *        or NULL to run in polling mode.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_EEPROM_Init(void (*callback)(void));

/**
 * @brief Disable the EEPROM module and clear EEIF.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_EEPROM_DeInit(void);

/**
 * @brief  Read one byte from data EEPROM. Loads EEADR with `addr`,
 *         sets EECON1<RD>, and returns the byte from EEDATA.
 *
 *         Per §3.5: EEADR must be loaded first, then RD.
 * @param addr the EEPROM address to read, 0..255.
 * @return the byte stored at `addr`.
 */
uint8_t EPIC_EEPROM_ReadByte(uint8_t addr);

/**
 * @brief  Write one byte to data EEPROM at `addr`. Performs the
 *         mandatory unlock sequence (0x55 → 0xAA → WR).
 * @param addr the EEPROM address to write, 0..255.
 * @param data the byte to store.
 * @return EPIC_OK on success, EPIC_ERROR if a previous
 *         write was aborted (WRERR set).
 */
EPIC_StatusTypeDef EPIC_EEPROM_WriteByte(uint8_t addr, uint8_t data);

/**
 * @brief Read a contiguous block.
 * @param start the first EEPROM address to read.
 * @param buf where the bytes are written.
 * @param len number of bytes to read.
 */
void EPIC_EEPROM_ReadBuffer(uint8_t start, uint8_t *buf, uint8_t len);

/**
 * @brief Write a contiguous block.
 * @param start the first EEPROM address to write.
 * @param buf the bytes to store.
 * @param len number of bytes to write.
 * @return EPIC_OK on success, EPIC_ERROR on a previous write error.
 */
EPIC_StatusTypeDef EPIC_EEPROM_WriteBuffer(uint8_t start,
                                                const uint8_t *buf,
                                                uint8_t len);

/**
 * @brief Returns 1 if EEIF is set.
 * @return 1 if the write-complete flag is set, 0 otherwise.
 */
uint8_t EPIC_EEPROM_IsWriteComplete(void);

/**
 * @brief Clear EEIF (must be cleared in the user's IRQ).
 */
void EPIC_EEPROM_ClearITFlag(void);

/**
 * @brief Weak EEPROM ISR, override in user code.
 */
void EEPROM_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_EEPROM_H */
