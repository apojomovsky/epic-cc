/*
 * Data EEPROM driver (DS39632E §7.0), 256 bytes. PIC18 moves the
 * registers into the Access Bank and adds EEPGD/CFGS in EECON1 (kept 0
 * for data EEPROM). The driver hides the 0x55/0xAA unlock sequence;
 * writes are non-blocking, the caller polls EEIF (PIR2<4>).
 */

#ifndef PIC18FXX5X_EEPROM_H
#define PIC18FXX5X_EEPROM_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief  Initialize the EEPROM driver. Programs PIE2<EEIE> if
 *         `callback` is non-NULL.
 * @param callback optional write-complete callback, or NULL for polling.
 * @return 0 on success.
 */
EPIC_StatusTypeDef EPIC_EEPROM_Init(void (*callback)(void));

/**
 * @brief Disable the EEPROM module and clear EEIF.
 * @return 0 on success.
 */
EPIC_StatusTypeDef EPIC_EEPROM_DeInit(void);

/**
 * @brief  Read one byte from data EEPROM. Loads EEADR with `addr`,
 *         sets EECON1<RD> (with EEPGD=0), and returns the byte from
 *         EEDATA. Per §7.1: EEADR must be loaded first, then RD.
 * @param addr the data EEPROM address, 0..255.
 * @return the byte stored at `addr`.
 */
uint8_t EPIC_EEPROM_ReadByte(uint8_t addr);

/**
 * @brief  Write one byte to data EEPROM at `addr`. Performs the mandatory
 *         unlock sequence (0x55 -> 0xAA -> WR), with EEPGD=0.
 * @param addr the data EEPROM address, 0..255.
 * @param data the byte to write.
 * @return EPIC_OK on success, EPIC_ERROR if a previous write was aborted
 *         (WRERR set).
 */
EPIC_StatusTypeDef EPIC_EEPROM_WriteByte(uint8_t addr, uint8_t data);

/**
 * @brief Read a contiguous block.
 * @param start the first data EEPROM address, 0..255.
 * @param buf where the bytes are written (must hold `len` bytes).
 * @param len the number of bytes to read.
 */
void EPIC_EEPROM_ReadBuffer(uint8_t start, uint8_t *buf, uint8_t len);

/**
 * @brief Write a contiguous block.
 * @param start the first data EEPROM address, 0..255.
 * @param buf the bytes to write (must hold `len` bytes).
 * @param len the number of bytes to write.
 * @return EPIC_OK on success, EPIC_ERROR if a write failed (WRERR set).
 */
EPIC_StatusTypeDef EPIC_EEPROM_WriteBuffer(uint8_t start,
                                              const uint8_t *buf,
                                              uint8_t len);

/**
 * @brief Returns 1 if EEIF is set (write cycle complete).
 * @return 1 when the write-complete flag is set, else 0.
 */
uint8_t EPIC_EEPROM_IsWriteComplete(void);

/**
 * @brief Clear EEIF (must be cleared in the user's IRQ).
 */
void EPIC_EEPROM_ClearITFlag(void);

/**
 * @brief EEPROM write-complete interrupt handler (weak default).
 */
void EEPROM_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_EEPROM_H */
