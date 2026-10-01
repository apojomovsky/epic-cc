/*
 * MSSP driver, implementation (DS39632E §19.0). Register-level: the I2C
 * state machine (Start/Stop/ACK, address matching) is left to the user.
 * All MSSP registers are in the Access Bank, no bank switching. RMW uses
 * split read+write because XC8 cannot lower a compound assignment on a
 * volatile cast-lvalue.
 */

#include "peripherals/pic18fxx5x_ssp.h"
#include "core/pic18_irq.h"

static SSP_HandleTypeDef        g_ssp_storage;
static const SSP_HandleTypeDef *g_ssp = NULL;

/**
 * @brief  Set bits in SSPCON2 using a split read+write (XC8 cannot lower
 *         a compound assignment on a volatile cast-lvalue).
 * @param mask Bitmask of SSPCON2 bits to set.
 */
static void sspcon2_set(uint8_t mask)
{
    uint8_t v = epic_sfr_read8(PIC_REG_SSPCON2);
    v |= (uint8_t)mask;
    epic_sfr_write8(PIC_REG_SSPCON2, v);
}

/**
 * @brief  Compute the SSPADD reload value for a target I2C clock:
 *         SSPADD = (Fosc / (4 * Fscl)) - 1.
 * @param fosc_hz System oscillator frequency in Hz.
 * @param fscl_hz Desired I2C clock frequency in Hz.
 * @return The 8-bit SSPADD value, or 0xFFFF if `fscl_hz` is 0 or the
 *         ratio exceeds 255.
 */
uint16_t SSP_ComputeSSPADD(uint32_t fosc_hz, uint32_t fscl_hz)
{
    if (fscl_hz == 0) return 0xFFFFU;
    uint32_t x = (fosc_hz / (4U * fscl_hz)) - 1U;
    if (x > 255U) return 0xFFFFU;
    return (uint16_t)x;
}

/**
 * @brief  Initialize the MSSP module from a handle. Programs SSPADD,
 *         SSPSTAT (SMP/CKE), SSPCON1 (mode, CKP, SSPEN) and clears
 *         SSPCON2, then arms the transfer interrupt if a callback is
 *         provided.
 * @param h Handle describing the MSSP configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_SSP_Init(const SSP_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    g_ssp_storage = *h;
    g_ssp = &g_ssp_storage;

    /* SSPADD (I2C slave address, or I2C master baud reload). */
    epic_sfr_write8(PIC_REG_SSPADD, h->SSPADD);

    /* SSPSTAT: SMP + CKE (SPI). */
    uint8_t stat = 0U;
    if (h->ClockEdge   == SSP_SPI_CKE_IDLE_ACTIVE) stat |= PIC_SSPSTAT_CKE;
    if (h->SamplePhase == SSP_SPI_SMP_END)        stat |= PIC_SSPSTAT_SMP;
    epic_sfr_write8(PIC_REG_SSPSTAT, stat);

    /* SSPCON1: SSPM3:0 (mode) + CKP + SSPEN. (WCOL/SSPOV are cleared by
     * writing the register; reset value 0x00.) */
    uint8_t con = (uint8_t)(h->Mode & PIC_SSPCON1_SSPM_MASK);
    if (h->ClockPolarity == SSP_SPI_CKP_IDLE_HIGH) con |= PIC_SSPCON1_CKP;
    con |= PIC_SSPCON1_SSPEN;
    epic_sfr_write8(PIC_REG_SSPCON1, con);

    /* SSPCON2: idle (all clear). */
    epic_sfr_write8(PIC_REG_SSPCON2, 0x00U);

    /* Interrupt enable. */
    EPIC_IRQ_ClearFlag(PIC18_IRQ_SSP);
    if (h->TransferCallback) EPIC_IRQ_Enable(PIC18_IRQ_SSP);
    else                     EPIC_IRQ_DisableSrc(PIC18_IRQ_SSP);

    return EPIC_OK;
}

/**
 * @brief  De-initialize the MSSP: disable its interrupt, clear the flag,
 *         zero SSPCON1/SSPCON2/SSPSTAT/SSPADD and drop the stored handle.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_SSP_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC18_IRQ_SSP);
    EPIC_IRQ_ClearFlag(PIC18_IRQ_SSP);
    epic_sfr_write8(PIC_REG_SSPCON1, 0x00U);
    epic_sfr_write8(PIC_REG_SSPCON2, 0x00U);
    epic_sfr_write8(PIC_REG_SSPSTAT, 0x00U);
    epic_sfr_write8(PIC_REG_SSPADD, 0x00U);
    g_ssp = NULL;
    return EPIC_OK;
}

/**
 * @brief  Write a byte to SSPBUF, starting a transfer. Fails fast if a
 *         write collision (WCOL) is pending.
 * @param data Byte to transmit.
 * @return 0 on success, 0xFFFF if WCOL was set (previous write still in
 *         progress).
 */
uint16_t EPIC_SSP_WriteByte(uint8_t data)
{
    uint8_t con = epic_sfr_read8(PIC_REG_SSPCON1);
    if (con & PIC_SSPCON1_WCOL) return 0xFFFFU;     /* write collision pending */
    epic_sfr_write8(PIC_REG_SSPBUF, data);
    /* On a real target the transfer starts and BF + SSPIF assert when it
     * completes; on the host sim, BF/SSPIF are set via the drive hook. */
    return 0U;
}

/**
 * @brief  Read a byte from SSPBUF; reading clears BF (DS39632E
 *         Register 19-1).
 * @return The received byte.
 */
uint8_t EPIC_SSP_ReadByte(void)
{
    /* Reading SSPBUF clears BF (DS39632E Register 19-1). */
    uint8_t v = epic_sfr_read8(PIC_REG_SSPBUF);
    uint8_t stat = (uint8_t)(epic_sfr_read8(PIC_REG_SSPSTAT) & (uint8_t)~PIC_SSPSTAT_BF);
    epic_sfr_write8(PIC_REG_SSPSTAT, stat);
    return v;
}

/**
 * @brief  Return 1 if the receive buffer is full (SSPSTAT<BF>).
 * @return 1 if BF is set, else 0.
 */
uint8_t EPIC_SSP_IsBufferFull(void)
{
    return (epic_sfr_read8(PIC_REG_SSPSTAT) & PIC_SSPSTAT_BF) ? 1U : 0U;
}

/**
 * @brief  Return 1 if a write collision (SSPCON1<WCOL>) is pending.
 * @return 1 if WCOL is set, else 0.
 */
uint8_t EPIC_SSP_HasWriteCollision(void)
{
    return (epic_sfr_read8(PIC_REG_SSPCON1) & PIC_SSPCON1_WCOL) ? 1U : 0U;
}

/**
 * @brief  Clear the write-collision flag (SSPCON1<WCOL>).
 */
void EPIC_SSP_ClearWriteCollision(void)
{
    uint8_t con = (uint8_t)(epic_sfr_read8(PIC_REG_SSPCON1) & (uint8_t)~PIC_SSPCON1_WCOL);
    epic_sfr_write8(PIC_REG_SSPCON1, con);
}

/**
 * @brief  Issue an I2C Start condition (SSPCON2<SEN>).
 */
void EPIC_SSP_Start(void)
{
    sspcon2_set(PIC_SSPCON2_SEN);
}
/**
 * @brief  Issue an I2C Repeated Start condition (SSPCON2<RSEN>).
 */
void EPIC_SSP_RepeatedStart(void)
{
    sspcon2_set(PIC_SSPCON2_RSEN);
}
/**
 * @brief  Issue an I2C Stop condition (SSPCON2<PEN>).
 */
void EPIC_SSP_Stop(void)
{
    sspcon2_set(PIC_SSPCON2_PEN);
}
/**
 * @brief  Enable I2C receive mode (SSPCON2<RCEN>).
 */
void EPIC_SSP_ReceiveEnable(void)
{
    sspcon2_set(PIC_SSPCON2_RCEN);
}
/**
 * @brief  Initiate an I2C Acknowledge sequence (SSPCON2<ACKEN>).
 */
void EPIC_SSP_AcknowledgeEnable(void)
{
    sspcon2_set(PIC_SSPCON2_ACKEN);
}

/**
 * @brief  Return the I2C Acknowledge status (SSPCON2<ACKSTAT>).
 * @return 1 if a NACK was received, 0 if an ACK was received.
 */
uint8_t EPIC_SSP_AcknowledgeStatus(void)
{
    return (epic_sfr_read8(PIC_REG_SSPCON2) & PIC_SSPCON2_ACKSTAT) ? 1U : 0U;
}

/**
 * @brief  Weak MSSP interrupt handler: clears SSPIF and invokes the
 *         transfer callback registered via Init.
 */
void SSP_IRQHandler(void)
{
    if (!EPIC_IRQ_GetFlag(PIC18_IRQ_SSP)) return;
    EPIC_IRQ_ClearFlag(PIC18_IRQ_SSP);
    if (g_ssp && g_ssp->TransferCallback) g_ssp->TransferCallback();
}
