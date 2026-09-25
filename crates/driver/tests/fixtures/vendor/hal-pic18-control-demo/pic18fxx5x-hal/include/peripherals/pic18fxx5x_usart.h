/*
 * EUSART driver, async + sync master/slave (DS39632E §20.0). PIC18 adds
 * a 16-bit BRG (BAUDCON<BRG16>, SPBRG:SPBRGH), auto-baud detect
 * (ABDEN/ABDOVF) and 9-bit address-detect (RCSTA<ADDEN>), all in the
 * Access Bank. RMW here uses split read+write because XC8 cannot lower
 * a compound assignment on a volatile cast-lvalue.
 */

#ifndef PIC18FXX5X_USART_H
#define PIC18FXX5X_USART_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief USART mode (TXSTA<SYNC>, DS39632E Register 20-1).
 */
typedef enum {
    USART_MODE_ASYNCHRONOUS  = 0x0U,   /**< SYNC = 0. */
    USART_MODE_SYNCHRONOUS   = 0x1U,   /**< SYNC = 1. */
} USART_ModeTypeDef;

/**
 * @brief Synchronous clock source (TXSTA<CSRC>, Register 20-1).
 *        Only meaningful in synchronous mode; ignored otherwise.
 */
typedef enum {
    USART_CLOCK_SLAVE        = 0x0U,   /**< CSRC = 0, clock from external. */
    USART_CLOCK_MASTER       = 0x1U,   /**< CSRC = 1, clock from BRG. */
} USART_ClockSourceTypeDef;

/**
 * @brief High/low baud-rate divisor (TXSTA<BRGH>, DS39632E §20.1).
 *        Combined with @ref USART_BaudGenTypeDef per Table 20-1.
 */
typedef enum {
    USART_BRGH_LOW           = 0x0U,   /**< BRGH = 0. */
    USART_BRGH_HIGH           = 0x1U,   /**< BRGH = 1. */
} USART_BaudRateHighTypeDef;

/**
 * @brief Baud-generator width (BAUDCON<BRG16>, DS39632E Register 20-3).
 *        BRG16=0 uses the 8-bit SPBRG; BRG16=1 uses the 16-bit
 *        SPBRG:SPBRGH pair, extending the divisor range to 0..65535.
 */
typedef enum {
    USART_BAUDGEN_8BIT       = 0x0U,   /**< BRG16 = 0, 8-bit BRG (SPBRG).   */
    USART_BAUDGEN_16BIT      = 0x1U,   /**< BRG16 = 1, 16-bit BRG (SPBRGH:SPBRG). */
} USART_BaudGenTypeDef;

/**
 * @brief Receive / transmit data width (RCSTA<RX9>, TXSTA<TX9>).
 */
typedef enum {
    USART_DATA_8BITS         = 0x0U,
    USART_DATA_9BITS         = 0x1U,
} USART_DataWidthTypeDef;

/**
 * @brief Compute the BRG divisor for a desired baud rate.
 *
 *   Async (Table 20-1):
 *     BRG16=0, BRGH=0: rate = FOSC / (64 × (X+1))
 *     BRG16=0, BRGH=1: rate = FOSC / (16 × (X+1))
 *     BRG16=1, BRGH=0: rate = FOSC / (16 × (X+1))
 *     BRG16=1, BRGH=1: rate = FOSC / (4  × (X+1))
 *   Sync:        rate = FOSC / (4  × (X+1))
 *
 * Returns 0..255 (BRG16=0) or 0..65535 (BRG16=1), or 0xFFFF if the
 * requested baud rate is unattainable (X would exceed the BRG range).
 * The caller splits the result into SPBRG (low byte) and SPBRGH (high
 * byte, BRG16=1 only).
 * @param fosc_hz the system oscillator frequency in Hz.
 * @param baud the desired baud rate in bits per second.
 * @param mode the USART mode (async or sync).
 * @param brgh the high/low baud-rate divisor select.
 * @param brg16 the baud-generator width (8- or 16-bit).
 * @return the BRG divisor (0..65535), or 0xFFFF if unattainable.
 */
uint16_t USART_ComputeSPBRG(uint32_t fosc_hz, uint32_t baud,
                            USART_ModeTypeDef mode,
                            USART_BaudRateHighTypeDef brgh,
                            USART_BaudGenTypeDef brg16);

/** Driver handle (Cube-style). */
typedef struct {
    USART_ModeTypeDef          Mode;
    USART_ClockSourceTypeDef   ClockSource;     /**< Sync only. */
    USART_BaudRateHighTypeDef  BaudHigh;
    USART_BaudGenTypeDef       BaudGen;          /**< BRG16: 8- or 16-bit BRG. */
    USART_DataWidthTypeDef     DataWidth;
    uint8_t                    SPBRG;            /**< 0..255, BRG low byte. */
    uint8_t                    SPBRGH;           /**< 0..255, BRG high byte (BRG16=1). */
    uint8_t                    AddressDetect;    /**< ADDEN: 9-bit address detect. */
    uint8_t                    AutoBaud;         /**< ABDEN: auto-baud detect on init. */
    /** @brief  Optional TX-complete callback (fires on TXIF). */
    void (*TxCpltCallback)(void);
    /** @brief  Optional RX-complete callback (fires on RCIF). */
    void (*RxCpltCallback)(uint8_t data);
} USART_HandleTypeDef;

#define USART_HANDLE_DEFAULT {                                          \
    .Mode            = USART_MODE_ASYNCHRONOUS,                         \
    .ClockSource     = USART_CLOCK_MASTER,                              \
    .BaudHigh        = USART_BRGH_HIGH,                                 \
    .BaudGen         = USART_BAUDGEN_8BIT,                              \
    .DataWidth       = USART_DATA_8BITS,                                \
    .SPBRG           = 0,                                               \
    .SPBRGH          = 0,                                               \
    .AddressDetect   = 0,                                               \
    .AutoBaud        = 0,                                               \
    .TxCpltCallback  = NULL,                                            \
    .RxCpltCallback  = NULL,                                            \
}

/**
 * @brief  Configure the EUSART: mode, baud-rate generator (SPBRG:SPBRGH),
 *         data width, address-detect and auto-baud, then enable the
 *         transmitter/receiver and optional callbacks.
 * @param h the USART handle describing the desired configuration.
 * @return 0 on success, 0xFFFF on invalid configuration.
 */
EPIC_StatusTypeDef EPIC_USART_Init(const USART_HandleTypeDef *h);

/**
 * @brief  Disable the EUSART (transmitter, receiver and interrupts).
 * @return 0 on success, 0xFFFF if the module was not initialized.
 */
EPIC_StatusTypeDef EPIC_USART_DeInit(void);

/**
 * @brief  Write one byte to TXREG. The write:
 *    - loads the byte into the TSR if it's empty (back-to-back transfer),
 *    - else parks it in TXREG until TSR drains,
 *    - clears TXIF (TXIF is read-only, cleared on TXREG write).
 *
 * @note   TXIF is NOT cleared by reading, only by writing TXREG.
 *         DS39632E §20.2.1.
 * @param data the byte to transmit.
 */
void EPIC_USART_Transmit(uint8_t data);

/**
 * @brief Read the 9th bit (TX9D) just transmitted.
 * @return the 9th data bit of the last transmitted byte.
 */
uint8_t EPIC_USART_GetTX9D(void);

/**
 * @brief Set the 9th bit to send NEXT. Must be set BEFORE writing TXREG.
 * @param bit9 the 9th data bit value (0 or 1).
 */
void EPIC_USART_SetTX9D(uint8_t bit9);

/**
 * @brief Returns 1 if the TSR is empty (TRMT = 1).
 * @return 1 when the transmit shift register is empty, else 0.
 */
uint8_t EPIC_USART_IsTxShiftRegisterEmpty(void);

/**
 * @brief  Read the latest byte from RCREG. Reading clears RCIF and
 *         advances the 2-deep FIFO.
 * @return the received byte.
 */
uint8_t EPIC_USART_Receive(void);

/**
 * @brief Read RX9D, the 9th bit of the most recently received byte.
 * @return the 9th data bit of the last received byte.
 */
uint8_t EPIC_USART_GetRX9D(void);

/**
 * @brief Returns 1 if an overrun was detected (RCSTA<OERR>). Clear it with
 *        @ref EPIC_USART_ClearOverrun (which cycles CREN).
 * @return 1 when an overrun occurred, else 0.
 */
uint8_t EPIC_USART_HasOverrun(void);

/**
 * @brief Clear an overrun: clear CREN, then re-set it (DS39632E §20.2.2).
 */
void EPIC_USART_ClearOverrun(void);

/**
 * @brief  Start auto-baud detection (sets BAUDCON<ABDEN>). The hardware
 *         measures the next incoming byte and loads SPBRG:SPBRGH; ABDEN
 *         self-clears when done. Poll @ref EPIC_USART_IsAutoBaudBusy.
 */
void EPIC_USART_StartAutoBaud(void);

/**
 * @brief Returns 1 while auto-baud detection is in progress (ABDEN = 1).
 * @return 1 while auto-baud detection is active, else 0.
 */
uint8_t EPIC_USART_IsAutoBaudBusy(void);

/**
 * @brief Returns 1 if auto-baud overflowed (ABDOVF set - measurement
 *        exceeded range).
 * @return 1 when the auto-baud measurement overflowed, else 0.
 */
uint8_t EPIC_USART_HasAutoBaudOverflow(void);

/**
 * @brief Clear the auto-baud overflow flag (ABDOVF).
 */
void EPIC_USART_ClearAutoBaudOverflow(void);

/**
 * @brief Weak USART RX ISR, override in user code.
 */
void USART_RX_IRQHandler(void) EPIC_WEAK;
/**
 * @brief Weak USART TX ISR, override in user code.
 */
void USART_TX_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_USART_H */
