/* USART driver, async + sync master/slave. Source: DS39582B §10.0,
 * §10.1 (BRG); full reference: MANUAL.md §14. One USART instance on
 * this family; 8/9-bit data, address-detect mode via ADDEN. */

#ifndef PIC16F87XA_USART_H
#define PIC16F87XA_USART_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief USART mode (TXSTA<SYNC>, DS39582B Register 10-1).
 */
typedef enum {
    USART_MODE_ASYNCHRONOUS  = 0x0U,   /**< SYNC = 0. */
    USART_MODE_SYNCHRONOUS   = 0x1U,   /**< SYNC = 1. */
} USART_ModeTypeDef;

/**
 * @brief Synchronous clock source (TXSTA<CSRC>, Register 10-1).
 *        Only meaningful in synchronous mode; ignored otherwise.
 */
typedef enum {
    USART_CLOCK_SLAVE        = 0x0U,   /**< CSRC = 0, clock from external. */
    USART_CLOCK_MASTER       = 0x1U,   /**< CSRC = 1, clock from BRG. */
} USART_ClockSourceTypeDef;

/**
 * @brief High/low baud-rate divisor (TXSTA<BRGH>, DS39582B §10.1).
 *        Async mode only. Synchronous mode always uses /4.
 */
typedef enum {
    USART_BRGH_LOW           = 0x0U,   /**< BRGH = 0, divisor 64. */
    USART_BRGH_HIGH          = 0x1U,   /**< BRGH = 1, divisor 16. */
} USART_BaudRateHighTypeDef;

/**
 * @brief Receive / transmit data width (RCSTA<RX9>, TXSTA<TX9>).
 */
typedef enum {
    USART_DATA_8BITS         = 0x0U,
    USART_DATA_9BITS         = 0x1U,
} USART_DataWidthTypeDef;

/**
 * @brief Compute the SPBRG value for a desired baud rate.
 *
 *   Async: rate = FOSC / (64 × (X+1))  (BRGH=0)
 *          rate = FOSC / (16 × (X+1))  (BRGH=1)
 *   Sync:  rate = FOSC / (4  × (X+1))
 *
 * Returns 0..255 (the SPBRG range), or 0xFFFF if the requested baud
 * rate is unattainable (X would have to exceed 255).
 * @param fosc_hz the oscillator frequency in Hz.
 * @param baud the desired baud rate in bits/s.
 * @param mode USART_MODE_ASYNCHRONOUS or USART_MODE_SYNCHRONOUS.
 * @param brgh USART_BRGH_LOW or USART_BRGH_HIGH (async only).
 * @return the SPBRG value 0..255, or 0xFFFF if unattainable.
 */
uint16_t USART_ComputeSPBRG(uint32_t fosc_hz, uint32_t baud,
                            USART_ModeTypeDef mode,
                            USART_BaudRateHighTypeDef brgh);

/** Driver handle (Cube-style). */
typedef struct {
    USART_ModeTypeDef          Mode;
    USART_ClockSourceTypeDef   ClockSource;
    USART_BaudRateHighTypeDef  BaudHigh;
    USART_DataWidthTypeDef     DataWidth;
    uint8_t                    SPBRG;        /**< 0..255, pre-computed. */
    /** @brief  Optional TX-complete callback (fires on TXIF). */
    void (*TxCpltCallback)(void);
    /** @brief  Optional RX-complete callback (fires on RCIF). */
    void (*RxCpltCallback)(uint8_t data);
} USART_HandleTypeDef;

#define USART_HANDLE_DEFAULT {                                          \
    .Mode          = USART_MODE_ASYNCHRONOUS,                           \
    .ClockSource   = USART_CLOCK_MASTER,                                \
    .BaudHigh      = USART_BRGH_HIGH,                                   \
    .DataWidth     = USART_DATA_8BITS,                                  \
    .SPBRG         = 0,                                                 \
    .TxCpltCallback = NULL,                                            \
    .RxCpltCallback = NULL,                                            \
}

/* init / deinit. */

/* The ISRs' owned callback slots, defined in the driver body. */
extern void (*g_usart_tx_cb)(void);
extern void (*g_usart_rx_cb)(uint8_t data);

#ifdef __EPIC_CC__
/* Static inline on the epic-cc path so the callback stores land in the
 * caller's TU as named literals, which the cross-context analysis
 * resolves (ADR-024, the timer0/timer2 precedent). XC8 keeps the
 * out-of-line driver body. */
#include "core/pic16_irq.h"

/**
 * @brief  Initialize the USART with the given handle. Programs TXSTA,
 *         RCSTA, SPBRG and the interrupt enables for the callbacks.
 * @param h handle with Mode, ClockSource, BaudHigh, DataWidth, SPBRG,
 *        TxCpltCallback, RxCpltCallback.
 * @return EPIC_OK on success, EPIC_ERROR if `h` is NULL.
 */
static inline EPIC_StatusTypeDef EPIC_USART_Init(const USART_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    g_usart_tx_cb = h->TxCpltCallback;
    g_usart_rx_cb = h->RxCpltCallback;

#ifdef EPIC_BANK1_WRITE8
    EPIC_BANK1_WRITE8(SPBRG, h->SPBRG);
#else
    uint8_t prev = (EPIC_REG8(PIC_REG_STATUS) >> 5) & 0x03U;
    pic_select_bank(1);
    EPIC_REG8(PIC_REG_SPBRG) = h->SPBRG;
    pic_select_bank(prev);
#endif

    uint8_t txsta = 0x02U;
    if (h->Mode == USART_MODE_SYNCHRONOUS) txsta |= PIC_TXSTA_SYNC;
    if (h->Mode == USART_MODE_SYNCHRONOUS &&
        h->ClockSource == USART_CLOCK_MASTER) txsta |= PIC_TXSTA_CSRC;
    if (h->BaudHigh == USART_BRGH_HIGH) txsta |= PIC_TXSTA_BRGH;
    if (h->DataWidth == USART_DATA_9BITS) txsta |= PIC_TXSTA_TX9;
    if (h->TxCpltCallback) txsta |= PIC_TXSTA_TXEN;
#ifdef EPIC_BANK1_WRITE8
    EPIC_BANK1_WRITE8(TXSTA, txsta);
#else
    EPIC_REG8(PIC_REG_TXSTA) = txsta;
#endif

    uint8_t rcsta = PIC_RCSTA_SPEN;
    if (h->DataWidth == USART_DATA_9BITS) rcsta |= PIC_RCSTA_RX9;
    if (h->RxCpltCallback) rcsta |= PIC_RCSTA_CREN;
    EPIC_REG8(PIC_REG_RCSTA) = rcsta;

    EPIC_IRQ_ClearFlag(PIC16_IRQ_USART_RX);

    if (h->TxCpltCallback) EPIC_IRQ_Enable(PIC16_IRQ_USART_TX);
    else                   EPIC_IRQ_DisableSrc(PIC16_IRQ_USART_TX);
    if (h->RxCpltCallback) EPIC_IRQ_Enable(PIC16_IRQ_USART_RX);
    else                   EPIC_IRQ_DisableSrc(PIC16_IRQ_USART_RX);

    return EPIC_OK;
}
#else
/**
 * @brief Initialize the USART.
 * @param h Handle.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_USART_Init(const USART_HandleTypeDef *h);
#endif
/**
 * @brief  De-initialize the USART. Disables the module and returns
 *         TXSTA/RCSTA to reset.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_USART_DeInit(void);

/* transmit. */

/**
 * @brief  Write one byte to TXREG. The write:
 *    - loads the byte into the TSR if it's empty (back-to-back transfer),
 *    - else parks it in TXREG until TSR drains,
 *    - sets TXIF = 0 (TXIF is read-only, cleared on TXREG write).
 *
 * @note   TXIF is NOT cleared by reading, only by writing TXREG.
 *         DS39582B §10.2.1.
 * @param data the byte to transmit.
 */
void EPIC_USART_Transmit(uint8_t data);

/**
 * @brief Read the 9th bit (TX9D) just transmitted.
 * @return the TX9D bit value.
 */
uint8_t EPIC_USART_GetTX9D(void);

/**
 * @brief Set the 9th bit to send NEXT. Must be set BEFORE writing TXREG.
 * @param bit9 the 9th data bit value (0 or 1).
 */
void EPIC_USART_SetTX9D(uint8_t bit9);

/**
 * @brief Returns 1 if the TSR is empty (TRMT = 1).
 * @return 1 if the transmit shift register is empty, 0 otherwise.
 */
uint8_t EPIC_USART_IsTxShiftRegisterEmpty(void);

/* receive. */

/**
 * @brief  Read the latest byte from RCREG. Reading:
 *    - clears RCIF,
 *    - advances the 2-deep FIFO.
 * @return the received byte.
 */
uint8_t EPIC_USART_Receive(void);

/**
 * @brief Read RX9D, the 9th bit of the most recently received byte.
 * @return the RX9D bit value.
 */
uint8_t EPIC_USART_GetRX9D(void);

/* interrupts. */

/**
 * @brief Weak USART RX ISR, override in user code.
 */
void USART_RX_IRQHandler(void) EPIC_WEAK;
/**
 * @brief Weak USART TX ISR, override in user code.
 */
void USART_TX_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_USART_H */
