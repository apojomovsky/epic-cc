/* MSSP driver implementation (DS39582B §9.0). Register-level only: the
 * I²C state machine (Start/Stop/ACK timing, slave address matching) is
 * left to the user; this configures SSPCON / SSPCON2 / SSPSTAT / SSPADD
 * and provides the byte-level transmit / receive primitives. */

#include "peripherals/pic16f87xa_ssp.h"
#include "core/pic16_irq.h"

/* handle storage. */

/* Owned copy of the caller's transfer callback for the weak ISR (the
 * caller's handle is typically stack-local, out of scope by the time
 * the ISR reads it). The ISR only needs the callback, so store the
 * pointer (1 byte) rather than a full handle copy, the same RAM
 * saving the 88X driver and both timer0 drivers make. */
static void (*g_ssp_transfer_cb)(void) = NULL;

/* SSPCON2/SSPSTAT/SSPADD are Bank 1 (SSPSTAT=0x94, SSPCON2=0x91,
 * SSPADD=0x93, DS39582B Figure 2-4). EPIC_BANK1_* need a literal SFR
 * name at compile time (inline-asm operands), not a runtime addr, so
 * this dispatches on `addr` before any bank switch, then invokes the
 * named macro; every real call site passes a compile-time constant, so
 * XC8 folds this to one branch either way. */
#ifdef EPIC_BANK1_READ8
/**
 * @brief Read a Bank-1 SSP register (SSPCON2, SSPADD or SSPSTAT).
 * @param addr the register address (0x91, 0x93 or 0x94).
 * @return the register value.
 */
static uint8_t ssp_b1_read(uint8_t addr)
{
    uint8_t v = 0U;
    if (addr == 0x91U)      EPIC_BANK1_READ8(SSPCON2, v);
    else if (addr == 0x93U) EPIC_BANK1_READ8(SSPADD, v);
    else                    EPIC_BANK1_READ8(SSPSTAT, v);
    return v;
}

/**
 * @brief Write a Bank-1 SSP register (SSPCON2, SSPADD or SSPSTAT).
 * @param addr the register address (0x91, 0x93 or 0x94).
 * @param v the byte to write.
 */
static void ssp_b1_write(uint8_t addr, uint8_t v)
{
    if (addr == 0x91U)      EPIC_BANK1_WRITE8(SSPCON2, v);
    else if (addr == 0x93U) EPIC_BANK1_WRITE8(SSPADD, v);
    else                    EPIC_BANK1_WRITE8(SSPSTAT, v);
}
#else
/**
 * @brief Read a Bank-1 SSP register via bank-switched EPIC_REG8 access.
 * @param addr the register address (0x91, 0x93 or 0x94).
 * @return the register value.
 */
static uint8_t ssp_b1_read(uint8_t addr)
{
    uint8_t prev = (EPIC_REG8(PIC_REG_STATUS) >> 5) & 0x03U;
    pic_select_bank(1);
    uint8_t v = EPIC_REG8(addr);
    pic_select_bank(prev);
    return v;
}

/**
 * @brief Write a Bank-1 SSP register via bank-switched EPIC_REG8 access.
 * @param addr the register address (0x91, 0x93 or 0x94).
 * @param v the byte to write.
 */
static void ssp_b1_write(uint8_t addr, uint8_t v)
{
    uint8_t prev = (EPIC_REG8(PIC_REG_STATUS) >> 5) & 0x03U;
    pic_select_bank(1);
    EPIC_REG8(addr) = v;
    pic_select_bank(prev);
}
#endif

/* SSPADD computation. */

/**
 * @brief Compute SSPADD for a target I²C clock.
 * @param fosc_hz the oscillator frequency in Hz.
 * @param fscl_hz the desired I²C clock in Hz.
 * @return the SSPADD reload value, or 0xFFFF if unattainable.
 */
uint16_t SSP_ComputeSSPADD(uint32_t fosc_hz, uint32_t fscl_hz)
{
    if (fscl_hz == 0) return 0xFFFFU;
    uint32_t x = (fosc_hz / (4U * fscl_hz)) - 1U;
    if (x > 255U) return 0xFFFFU;
    return (uint16_t)x;
}

/* public API. */

/**
 * @brief Initialize the MSSP: program SSPADD, SSPSTAT, SSPCON and
 *        SSPCON2 from the handle and arm the transfer interrupt.
 * @param h handle with Mode, ClockEdge, ClockPolarity, SamplePhase,
 *        SSPADD, TransferCallback.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_SSP_Init(const SSP_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    g_ssp_transfer_cb = h->TransferCallback;

    /* Program SSPADD (Bank 1, address 0x93). */
    ssp_b1_write(0x93U, h->SSPADD);

    /* Build SSPSTAT (Bank 1, address 0x94). */
    uint8_t stat = 0U;
    if (h->ClockEdge   == SSP_SPI_CKE_IDLE_ACTIVE) stat |= PIC_SSPSTAT_CKE;
    if (h->SamplePhase == SSP_SPI_SMP_END)        stat |= PIC_SSPSTAT_SMP;
    ssp_b1_write(0x94U, stat);

    /* Build SSPCON (Bank 0, address 0x14):
     *   bit 0..3 SSPM3:SSPM0 (mode)
     *   bit 4   CKP
     *   bit 5   SSPEN
     *   bit 6   SSPOV (read-only, cleared by software)
     *   bit 7   WCOL  (read-only, cleared by software)
     * Reset value: 0x00. */
    uint8_t con = (uint8_t)(h->Mode & 0x0FU);
    if (h->ClockPolarity == SSP_SPI_CKP_IDLE_HIGH) con |= PIC_SSPCON_CKP;
    con |= PIC_SSPCON_SSPEN;
    EPIC_REG8(0x14U) = con;

    /* SSPCON2 (Bank 1, address 0x91), clear all bits (idle state). */
    ssp_b1_write(0x91U, 0x00U);

    /* Interrupt enable. */
    EPIC_IRQ_ClearFlag(PIC16_IRQ_SSP);
    if (h->TransferCallback) EPIC_IRQ_Enable(PIC16_IRQ_SSP);
    else                     EPIC_IRQ_DisableSrc(PIC16_IRQ_SSP);

    return EPIC_OK;
}

/**
 * @brief De-initialize the MSSP: disable the interrupt and reset
 *        SSPCON/SSPCON2/SSPSTAT/SSPADD.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_SSP_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC16_IRQ_SSP);
    EPIC_IRQ_ClearFlag(PIC16_IRQ_SSP);
    EPIC_REG8(0x14U) = 0x00U;
    ssp_b1_write(0x91U, 0x00U);
    ssp_b1_write(0x94U, 0x00U);
    ssp_b1_write(0x93U, 0x00U);
    g_ssp_transfer_cb = NULL;
    return EPIC_OK;
}

/**
 * @brief Write a byte to SSPBUF.
 * @param data the byte to transmit.
 * @return 0 on success, 0xFFFF if a write collision is pending.
 */
uint16_t EPIC_SSP_WriteByte(uint8_t data)
{
    uint8_t con = EPIC_REG8(0x14U);
    if (con & PIC_SSPCON_WCOL) return 0xFFFFU;     /* write collision pending. */
    EPIC_REG8(PIC_REG_SSPBUF) = data;
    /* The sim backend sets BF + SSPIF on the next sim_step
     * (see sim_step_ssp() in src/sim/pic16f87xa_sim.c). */
    return 0U;
}

/**
 * @brief Read the received byte from SSPBUF and clear BF.
 * @return the byte in SSPBUF.
 */
uint8_t EPIC_SSP_ReadByte(void)
{
    /* Reading SSPBUF clears BF (Register 9-1). */
    uint8_t v = EPIC_REG8(PIC_REG_SSPBUF);
#ifdef EPIC_BANK1_READ8
    /* Plain EPIC_REG8 RMW on Bank-1 SSPSTAT (0x94) misdirects to the
     * Bank-0 alias (0x14, SSPCON) under XC8 v4.00, clearing CKP instead
     * of BF (see target/pic16f87xa_platform.h). */
    {
        uint8_t stat = 0u;
        EPIC_BANK1_READ8(SSPSTAT, stat);
        stat &= (uint8_t)~PIC_SSPSTAT_BF;
        EPIC_BANK1_WRITE8(SSPSTAT, stat);
    }
#else
    EPIC_REG8(0x94U) &= (uint8_t)~PIC_SSPSTAT_BF;
#endif
    return v;
}

/**
 * @brief Report whether SSPBUF holds an unread byte.
 * @return 1 if BF is set, 0 otherwise.
 */
uint8_t EPIC_SSP_IsBufferFull(void)
{
#ifdef EPIC_BANK1_READ8
    uint8_t stat = 0u;
    EPIC_BANK1_READ8(SSPSTAT, stat);
    return (stat & PIC_SSPSTAT_BF) ? 1U : 0U;
#else
    return (EPIC_REG8(0x94U) & PIC_SSPSTAT_BF) ? 1U : 0U;
#endif
}

/**
 * @brief Report whether a write collision was detected.
 * @return 1 if WCOL is set, 0 otherwise.
 */
uint8_t EPIC_SSP_HasWriteCollision(void)
{
    return (EPIC_REG8(0x14U) & PIC_SSPCON_WCOL) ? 1U : 0U;
}

/**
 * @brief Clear the write-collision flag.
 */
void EPIC_SSP_ClearWriteCollision(void)
{
    EPIC_REG8(0x14U) &= (uint8_t)~PIC_SSPCON_WCOL;
}

/**
 * @brief Issue a Start condition (SSPCON2<SEN>).
 */
void EPIC_SSP_Start(void)
{
    /* Writing SSPCON2<SEN>=1 initiates a Start. The hardware clears
     * the bit when the Start completes. (§9.4.2) */
    ssp_b1_write(0x91U, ssp_b1_read(0x91U) | PIC_SSPCON2_SEN);
}

/**
 * @brief Issue a Repeated Start condition (SSPCON2<RSEN>).
 */
void EPIC_SSP_RepeatedStart(void)
{
    ssp_b1_write(0x91U, ssp_b1_read(0x91U) | PIC_SSPCON2_RSEN);
}

/**
 * @brief Issue a Stop condition (SSPCON2<PEN>).
 */
void EPIC_SSP_Stop(void)
{
    ssp_b1_write(0x91U, ssp_b1_read(0x91U) | PIC_SSPCON2_PEN);
}

/**
 * @brief Begin a master receive (SSPCON2<RCEN>).
 */
void EPIC_SSP_ReceiveEnable(void)
{
    ssp_b1_write(0x91U, ssp_b1_read(0x91U) | PIC_SSPCON2_RCEN);
}

/**
 * @brief Transmit an ACK for the received byte (SSPCON2<ACKEN>).
 */
void EPIC_SSP_AcknowledgeEnable(void)
{
    /* Begin ACK sequence: SSPCON2<ACKEN>=1, ACKDT holds the bit value. */
    ssp_b1_write(0x91U, ssp_b1_read(0x91U) | PIC_SSPCON2_ACKEN);
}

/**
 * @brief Report whether the slave acknowledged.
 * @return 1 if ACKSTAT is set (slave ACKed), 0 otherwise.
 */
uint8_t EPIC_SSP_AcknowledgeStatus(void)
{
    return (ssp_b1_read(0x91U) & PIC_SSPCON2_ACKSTAT) ? 1U : 0U;
}

/* ISRs. */

/**
 * @brief Weak SSP ISR: clears SSPIF and fires the transfer callback.
 */
void SSP_IRQHandler(void)
{
    /* Direct flag ops (class-F: the table route clobbers PCLATH in ISR
     * context; see the CCP handlers). SSPIF is PIR1 bit 3. */
    if (!(EPIC_REG8(PIC_REG_PIR1) & PIC_PIR1_SSPIF)) return;
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_SSPIF);
    if (g_ssp_transfer_cb) g_ssp_transfer_cb();
}
