/* MSSP driver, SPI master/slave + I²C master/slave. Source: DS39582B
 * §9.0; full register reference: MANUAL.md §15. Register-level only, no
 * I²C state machine: a master issues Start, writes SSPBUF, polls
 * SSPSTAT<BF> + ACKSTAT (SSPCON2<6>), then Stop. SPI is automatic once
 * SSPBUF is written; poll SSPSTAT<BF> for RX-ready. */

#ifndef PIC16F87XA_SSP_H
#define PIC16F87XA_SSP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief SSP mode select (SSPCON<3:0>, Registers 9-2 / 9-4).
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
 * @brief SPI clock-edge select (SSPSTAT<CKE>, §9.3.1 / Table 9-1).
 *        Only meaningful in SPI mode.
 */
typedef enum {
    SSP_SPI_CKE_ACTIVE_IDLE = 0x0U,   /**< CKE=0: transmit on idle→active. */
    SSP_SPI_CKE_IDLE_ACTIVE = 0x1U,   /**< CKE=1: transmit on active→idle. */
} SSP_ClockEdgeTypeDef;

/**
 * @brief SPI clock polarity (SSPCON<CKP>, §9.3.5).
 */
typedef enum {
    SSP_SPI_CKP_IDLE_LOW   = 0x0U,
    SSP_SPI_CKP_IDLE_HIGH  = 0x1U,
} SSP_ClockPolarityTypeDef;

/**
 * @brief SPI input-data sample phase (SSPSTAT<SMP>, §9.3.5).
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

/* init / deinit. */

/**
 * @brief  Initialize the MSSP module with the given handle. Programs
 *         SSPCON/SSPSTAT (mode, clock edge/polarity/sample) and installs
 *         the transfer callback.
 * @param h handle with Mode, ClockEdge, ClockPolarity, SamplePhase,
 *        SSPADD, TransferCallback.
 * @return EPIC_OK on success, EPIC_ERROR if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_SSP_Init(const SSP_HandleTypeDef *h);

/**
 * @brief  De-initialize the MSSP module. Disables the module, clears
 *         the callback and returns SSPCON/SSPSTAT to reset.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_SSP_DeInit(void);

/* SPI transfer. */

/**
 * @brief  Write a byte to SSPBUF (and thus to the SSPSR if idle).
 *         Returns 0xFFFF if WCOL (write collision) was set, in which
 *         case the byte was *not* written and the user should retry.
 * @param data the byte to transmit.
 * @return 0xFFFF on write collision, else the received byte in the
 *         low 8 bits (undefined for a pure transmit).
 */
uint16_t EPIC_SSP_WriteByte(uint8_t data);

/**
 * @brief Read the most recently received byte from SSPBUF.
 * @return the byte in SSPBUF.
 */
uint8_t  EPIC_SSP_ReadByte(void);

/**
 * @brief Returns 1 if SSPBUF holds an unread byte (BF = 1).
 * @return 1 if the buffer holds data, 0 otherwise.
 */
uint8_t  EPIC_SSP_IsBufferFull(void);

/**
 * @brief Returns 1 if a write collision was detected.
 * @return 1 if WCOL is set, 0 otherwise.
 */
uint8_t  EPIC_SSP_HasWriteCollision(void);

/**
 * @brief Clear the WCOL flag (must be done in software per §9.3.2).
 */
void     EPIC_SSP_ClearWriteCollision(void);

/* I²C master helpers. */

/**
 * @brief  Compute SSPADD for an I²C master baud rate.
 *         DS39582B §9.4.2: Fscl = Fosc / (4 × (SSPADD + 1))
 *         → SSPADD = (Fosc / (4 × Fscl)) - 1.
 * @param fosc_hz the oscillator frequency in Hz.
 * @param fscl_hz the desired I²C clock frequency in Hz.
 * @return the SSPADD value to load, or 0xFFFF if `fscl_hz` is 0.
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
 * @return 1 if the slave acknowledged, 0 otherwise.
 */
uint8_t EPIC_SSP_AcknowledgeStatus(void);

/* interrupts. */

/**
 * @brief Weak SSP ISR, override in user code.
 */
void SSP_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_SSP_H */
