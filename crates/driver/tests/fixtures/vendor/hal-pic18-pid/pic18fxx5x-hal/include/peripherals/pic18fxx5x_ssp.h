/*
 * MSSP driver: SPI master/slave + I2C master/slave (DS39632E §19.0).
 * The PIC18 MSSP registers are all in the Access Bank (no bank switching)
 * and the control register is SSPCON1 (PIC16's is SSPCON). Register-level
 * only: the I2C Start/Stop/ACK state machine is left to the caller; SPI
 * transfers complete automatically once SSPBUF is written, poll
 * SSPSTAT<BF> for a byte's arrival.
 */

#ifndef PIC18FXX5X_SSP_H
#define PIC18FXX5X_SSP_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief SSP mode select (SSPCON1<3:0>, DS39632E Registers 19-2/19-4).
 */
typedef enum {
    SSP_MODE_SPI_MASTER_FOSC_4   = 0x0U,   /**< 0000, SPI master, Fosc/4.   */
    SSP_MODE_SPI_MASTER_FOSC_16  = 0x1U,   /**< 0001, SPI master, Fosc/16.  */
    SSP_MODE_SPI_MASTER_FOSC_64  = 0x2U,   /**< 0010, SPI master, Fosc/64.  */
    SSP_MODE_SPI_MASTER_TMR2     = 0x3U,   /**< 0011, SPI master, TMR2/2.   */
    SSP_MODE_SPI_SLAVE_SS_DIS    = 0x4U,   /**< 0100, SPI slave, SS disabled. */
    SSP_MODE_SPI_SLAVE_SS_EN     = 0x5U,   /**< 0101, SPI slave, SS enabled. */
    SSP_MODE_I2C_SLAVE_7BIT      = 0x6U,   /**< 0110, I²C slave, 7-bit. */
    SSP_MODE_I2C_SLAVE_10BIT     = 0x7U,   /**< 0111, I²C slave, 10-bit. */
    SSP_MODE_I2C_MASTER_FW       = 0x8U,   /**< 1000, I²C master, firmware. */
    SSP_MODE_I2C_SLAVE_7BIT_SS   = 0x9U,   /**< 1001, I²C slave, 7-bit + S/S. */
    SSP_MODE_I2C_SLAVE_10BIT_SS  = 0xAU,   /**< 1010, I²C slave, 10-bit + S/S. */
    SSP_MODE_I2C_MASTER_FOSC     = 0xBU,   /**< 1011, I²C master, hardware. */
} SSP_ModeTypeDef;

/**
 * @brief SPI clock-edge select (SSPSTAT<CKE>, DS39632E §19.2.1).
 */
typedef enum {
    SSP_SPI_CKE_ACTIVE_IDLE = 0x0U,   /**< CKE=0: transmit on idle->active. */
    SSP_SPI_CKE_IDLE_ACTIVE = 0x1U,   /**< CKE=1: transmit on active->idle. */
} SSP_ClockEdgeTypeDef;

/**
 * @brief SPI clock polarity (SSPCON1<CKP>, DS39632E §19.2.4).
 */
typedef enum {
    SSP_SPI_CKP_IDLE_LOW   = 0x0U,
    SSP_SPI_CKP_IDLE_HIGH  = 0x1U,
} SSP_ClockPolarityTypeDef;

/**
 * @brief SPI input-data sample phase (SSPSTAT<SMP>, DS39632E §19.2.4).
 *        Master mode only.
 */
typedef enum {
    SSP_SPI_SMP_MIDDLE = 0x0U,   /**< SMP=0: sample at middle of data. */
    SSP_SPI_SMP_END    = 0x1U,   /**< SMP=1: sample at end of data. */
} SSP_SamplePhaseTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    SSP_ModeTypeDef           Mode;
    SSP_ClockEdgeTypeDef      ClockEdge;       /**< SPI only. */
    SSP_ClockPolarityTypeDef  ClockPolarity;   /**< SPI only. */
    SSP_SamplePhaseTypeDef    SamplePhase;     /**< SPI master only. */
    uint8_t                   SSPADD;          /**< I²C slave address or master baud reload. */
    /** @brief Optional transfer callback (fires on SSPIF). */
    void (*TransferCallback)(void);
} SSP_HandleTypeDef;

#define SSP_HANDLE_DEFAULT {                                              \
    .Mode          = SSP_MODE_SPI_MASTER_FOSC_4,                           \
    .ClockEdge     = SSP_SPI_CKE_IDLE_ACTIVE,                              \
    .ClockPolarity = SSP_SPI_CKP_IDLE_LOW,                                 \
    .SamplePhase   = SSP_SPI_SMP_MIDDLE,                                   \
    .SSPADD        = 0,                                                    \
    .TransferCallback = NULL,                                              \
}

/**
 * @brief  Configure the MSSP module: mode, SPI clock edge/polarity/sample
 *         phase and SSPADD, then enable the module and (optionally) the
 *         SSPIF interrupt.
 * @param h the SSP handle describing the desired configuration.
 * @return 0 on success, 0xFFFF on invalid configuration.
 */
EPIC_StatusTypeDef EPIC_SSP_Init(const SSP_HandleTypeDef *h);

/**
 * @brief  Disable the MSSP module and clear SSPIF.
 * @return 0 on success, 0xFFFF if the module was not initialized.
 */
EPIC_StatusTypeDef EPIC_SSP_DeInit(void);

/**
 * @brief  Write a byte to SSPBUF. Returns 0xFFFF if WCOL (write collision)
 *         was set, in which case the byte was *not* written; retry.
 * @param data the byte to transmit.
 * @return 0 on success, 0xFFFF on write collision.
 */
uint16_t EPIC_SSP_WriteByte(uint8_t data);

/**
 * @brief Read the most recently received byte from SSPBUF (clears BF).
 * @return the received byte.
 */
uint8_t  EPIC_SSP_ReadByte(void);

/**
 * @brief Returns 1 if SSPBUF holds an unread byte (BF = 1).
 * @return 1 when a byte is available, else 0.
 */
uint8_t  EPIC_SSP_IsBufferFull(void);

/**
 * @brief Returns 1 if a write collision was detected.
 * @return 1 when WCOL is set, else 0.
 */
uint8_t  EPIC_SSP_HasWriteCollision(void);

/**
 * @brief Clear the WCOL flag (must be done in software per §19.2.2).
 */
void     EPIC_SSP_ClearWriteCollision(void);

/**
 * @brief  Compute SSPADD for an I²C master baud rate.
 *         DS39632E §19.4.2: Fscl = Fosc / (4 x (SSPADD + 1))
 *         -> SSPADD = (Fosc / (4 x Fscl)) - 1.
 * @param fosc_hz the system oscillator frequency in Hz.
 * @param fscl_hz the desired I²C clock frequency in Hz.
 * @return the SSPADD reload value.
 */
uint16_t SSP_ComputeSSPADD(uint32_t fosc_hz, uint32_t fscl_hz);

/**
 * @brief Issue a Start condition (sets SSPCON2<SEN>).
 */
void EPIC_SSP_Start(void);

/**
 * @brief Issue a Repeated Start condition.
 */
void EPIC_SSP_RepeatedStart(void);

/**
 * @brief Issue a Stop condition.
 */
void EPIC_SSP_Stop(void);

/**
 * @brief Begin a receive (master mode). Sets SSPCON2<RCEN>.
 */
void EPIC_SSP_ReceiveEnable(void);

/**
 * @brief Transmit an ACK (master receive).
 */
void EPIC_SSP_AcknowledgeEnable(void);

/**
 * @brief Returns 1 if an ACK was received from the slave (ACKSTAT).
 * @return 1 when the slave acknowledged, else 0.
 */
uint8_t EPIC_SSP_AcknowledgeStatus(void);

/**
 * @brief MSSP transfer interrupt handler (weak default).
 */
void SSP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_SSP_H */
