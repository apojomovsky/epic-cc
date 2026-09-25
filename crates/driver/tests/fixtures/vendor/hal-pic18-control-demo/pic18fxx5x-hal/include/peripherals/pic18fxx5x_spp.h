/*
 * Streaming Parallel Port (DS39632E §18.0), 40/44-pin only (gated by
 * `PIC18FXX5X_FAMILY_HAS_SPP`): a USB-era parallel port, the PIC18 analog
 * of PIC16's PSP. Register-level only: programs SPPCON/SPPCFG/SPPEPS and
 * provides byte-level SPPDATA access plus the busy/read/write status
 * flags and SPPIF; the USB streaming protocol is left to the user.
 */

#ifndef PIC18FXX5X_SPP_H
#define PIC18FXX5X_SPP_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

#if !PIC18FXX5X_FAMILY_HAS_SPP
#error "pic18fxx5x_spp.h is for 40/44-pin PIC18FXX5X parts only (4455/4550)"
#endif

/**
 * @brief SPP ownership (SPPCON<SPPOWN>, Register 18-1).
 */
typedef enum {
    SPP_OWN_MICROCONTROLLER = 0x0U,   /**< SPPOWN=0: MCU directly controls SPP. */
    SPP_OWN_USB             = 0x1U,   /**< SPPOWN=1: USB peripheral controls SPP. */
} SPP_OwnershipTypeDef;

/**
 * @brief SPP clock configuration (SPPCFG<CLKCFG1:CLKCFG0>, Register 18-2).
 *        The driver shifts these into bits 7:6.
 */
typedef enum {
    SPP_CLKCFG_ADDR_WRITE_DATA_RW = 0x0U,   /**< 00: CLK1 on addr write, CLK2 on data R/W. */
    SPP_CLKCFG_WRITE_READ         = 0x1U,   /**< 01: CLK1 on write, CLK2 on read.    */
    SPP_CLKCFG_ODD_EVEN          = 0x2U,   /**< 1x: CLK1 on odd addr, CLK2 on even. */
} SPP_ClockConfigTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    SPP_OwnershipTypeDef       Ownership;
    SPP_ClockConfigTypeDef      ClockConfig;
    bool                        CSEnable;     /**< CSEN: RB4 as SPP CS output. */
    bool                        CLK1Enable;   /**< CLK1EN: RE0 as SPP CLK1 output. */
    uint8_t                     WaitStates;   /**< WS3:WS0, 0..15 (-> 0..30 wait states). */
    uint8_t                     Endpoint;     /**< ADDR3:ADDR0, 0..15. */
    /** @brief Optional transfer callback (fires on SPPIF). */
    void (*TransferCallback)(void);
} SPP_HandleTypeDef;

#define SPP_HANDLE_DEFAULT {                                              \
    .Ownership        = SPP_OWN_MICROCONTROLLER,                          \
    .ClockConfig     = SPP_CLKCFG_ADDR_WRITE_DATA_RW,                      \
    .CSEnable        = false,                                              \
    .CLK1Enable      = false,                                              \
    .WaitStates      = 0,                                                  \
    .Endpoint        = 0,                                                  \
    .TransferCallback = NULL,                                             \
}

/**
 * @brief  Configure the Streaming Parallel Port: ownership, clock config,
 *         CS/CLK1 enables, wait states and endpoint, then enable the
 *         module (SPPCON<SPPEN>).
 * @param h the SPP handle describing the desired configuration.
 * @return 0 on success, 0xFFFF on invalid configuration.
 */
EPIC_StatusTypeDef EPIC_SPP_Init(const SPP_HandleTypeDef *h);

/**
 * @brief  Disable the SPP module (SPPCON<SPPEN> = 0) and clear SPPIF.
 * @return 0 on success, 0xFFFF if the module was not initialized.
 */
EPIC_StatusTypeDef EPIC_SPP_DeInit(void);

/**
 * @brief  Write a byte to SPPDATA for endpoint `ep` (sets SPPEPS<ADDR>
 *         first, then loads SPPDATA). The CLK/CS outputs strobe per the
 *         SPPCFG configuration.
 * @param ep the endpoint address, 0..15.
 * @param data the byte to write.
 */
void EPIC_SPP_WriteByte(uint8_t ep, uint8_t data);

/**
 * @brief  Read a byte from SPPDATA for endpoint `ep`. Returns the byte.
 * @param ep the endpoint address, 0..15.
 * @return the byte read from SPPDATA.
 */
uint8_t EPIC_SPP_ReadByte(uint8_t ep);

/**
 * @brief Returns 1 if the SPP is busy (SPPEPS<SPPBUSY>).
 * @return 1 while the SPP is busy, else 0.
 */
uint8_t EPIC_SPP_IsBusy(void);

/**
 * @brief Returns 1 if a write occurred since the flag was last cleared
 *        (SPPEPS<WRSPP>).
 * @return 1 when a write has occurred, else 0.
 */
uint8_t EPIC_SPP_HasWriteOccurred(void);

/**
 * @brief Returns 1 if a read occurred since the flag was last cleared
 *        (SPPEPS<RDSPP>).
 * @return 1 when a read has occurred, else 0.
 */
uint8_t EPIC_SPP_HasReadOccurred(void);

/**
 * @brief Returns 1 if SPPIF is set.
 * @return 1 when the SPP interrupt flag is set, else 0.
 */
uint8_t EPIC_SPP_IsInterruptFlag(void);

/**
 * @brief Clear the SPPIF flag (must be done in the transfer IRQ).
 */
void EPIC_SPP_ClearITFlag(void);

/**
 * @brief SPP transfer interrupt handler (weak default).
 */
void SPP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_SPP_H */
