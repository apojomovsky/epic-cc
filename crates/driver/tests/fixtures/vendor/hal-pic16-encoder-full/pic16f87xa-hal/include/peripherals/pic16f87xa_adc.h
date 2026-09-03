/* A/D converter driver, 10-bit, 5/8 channels. Source: DS39582B §11.0;
 * full register/wiring reference: pic16f87xa-hal/MANUAL.md §16. The
 * acquisition time (§11.1) between channel select and GO is enforced by
 * requiring an explicit EPIC_ADC_Start() call. */

#ifndef PIC16F87XA_ADC_H
#define PIC16F87XA_ADC_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief A/D channel (ADCON0<CHS2:CHS0>, Register 11-1).
 *        AN0..AN4 valid on all parts; AN5..AN7 only on 40/44-pin.
 */
typedef enum {
    ADC_CHANNEL_AN0 = 0x0U,
    ADC_CHANNEL_AN1 = 0x1U,
    ADC_CHANNEL_AN2 = 0x2U,
    ADC_CHANNEL_AN3 = 0x3U,
    ADC_CHANNEL_AN4 = 0x4U,
    ADC_CHANNEL_AN5 = 0x5U,   /* 40/44-pin only. */
    ADC_CHANNEL_AN6 = 0x6U,   /* 40/44-pin only. */
    ADC_CHANNEL_AN7 = 0x7U,   /* 40/44-pin only. */
} ADC_ChannelTypeDef;

/**
 * @brief A/D clock source (ADCON1<ADCS2> + ADCON0<ADCS1:ADCS0>, Tables
 *        11-1 and Register 11-1).  The driver combines the two into a
 *        single 3-bit value.
 *
 *        Each value carries a per-datasheet Tad.  The 2/4/8/16/32/64
 *        Tosc options assume Fosc ≤ 1.25 / 2.5 / 5 / 10 / 20 / 20 MHz
 *        respectively (Table 11-1).
 */
typedef enum {
    ADC_CLOCK_FOSC_2     = 0x0U,   /**< 000, Fosc/2.   */
    ADC_CLOCK_FOSC_8     = 0x1U,   /**< 001, Fosc/8.   */
    ADC_CLOCK_FOSC_32    = 0x2U,   /**< 010, Fosc/32.  */
    ADC_CLOCK_RC         = 0x3U,   /**< 011, Internal A/D RC. */
    ADC_CLOCK_FOSC_4     = 0x4U,   /**< 100, Fosc/4.   */
    ADC_CLOCK_FOSC_16    = 0x5U,   /**< 101, Fosc/16.  */
    ADC_CLOCK_FOSC_64    = 0x6U,   /**< 110, Fosc/64.  */
    /* 111 = RC, same as 011, kept reserved. */
} ADC_ClockSourceTypeDef;

/**
 * @brief Result-format select (ADCON1<ADFM>, Register 11-2).
 */
typedef enum {
    ADC_FORMAT_LEFT  = 0x0U,   /**< ADFM=0, left justified. */
    ADC_FORMAT_RIGHT = 0x1U,   /**< ADFM=1, right justified. */
} ADC_ResultFormatTypeDef;

/**
 * @brief Voltage reference configuration (ADCON1<PCFG3:PCFG0>, §11.0
 *        Table in Register 11-2).  The 4-bit value maps directly to
 *        the datasheet's configuration table.
 */
typedef enum {
    ADC_REFERENCE_VDD_VSS_8CH    = 0x0U,   /* AN0..AN7 analog, Vref+=Vdd, Vref-=Vss. */
    ADC_REFERENCE_VDD_VSS_7CH    = 0x1U,   /* AN0..AN6 + Vref-=AN3. */
    ADC_REFERENCE_VDD_VSS_5CH    = 0x2U,   /* AN0..AN4, Vref+ on AN3. */
    ADC_REFERENCE_VDD_VSS_4CH    = 0x3U,   /* AN0..AN3, Vref+/Vref- on AN3/AN2. */
    ADC_REFERENCE_VDD_VSS_3CH    = 0x4U,   /* AN0..AN2. */
    ADC_REFERENCE_VREF_2CH      = 0x5U,   /* AN0..AN1 + Vref+/Vref- on AN3/AN2. */
    ADC_REFERENCE_VDD_VSS_6CH    = 0x6U,   /* AN0..AN5. */
    ADC_REFERENCE_VDD_VSS_5CH_B  = 0x7U,
    ADC_REFERENCE_VREF_6CH      = 0x8U,   /* AN0..AN5, Vref+/Vref- on AN3/AN2. */
    ADC_REFERENCE_VDD_VSS_6CH_C  = 0x9U,
    ADC_REFERENCE_VDD_VSS_5CH_D  = 0xAU,
    ADC_REFERENCE_VREF_4CH       = 0xBU,
    ADC_REFERENCE_VREF_3CH       = 0xCU,
    ADC_REFERENCE_VREF_2CH_B     = 0xDU,
    ADC_REFERENCE_VDD_VSS_2CH    = 0xEU,
    ADC_REFERENCE_VREF_2CH_C     = 0xFU,
} ADC_ReferenceTypeDef;

/** Driver handle (Cube-style). */
typedef struct {
    ADC_ChannelTypeDef          Channel;
    ADC_ClockSourceTypeDef      ClockSource;
    ADC_ResultFormatTypeDef     ResultFormat;
    ADC_ReferenceTypeDef        Reference;
    /** @brief Optional conversion-complete callback (fires on ADIF). */
    void (*ConvCpltCallback)(uint16_t result);
} ADC_HandleTypeDef;

#define ADC_HANDLE_DEFAULT {                                              \
    .Channel         = ADC_CHANNEL_AN0,                                    \
    .ClockSource     = ADC_CLOCK_FOSC_8,                                   \
    .ResultFormat    = ADC_FORMAT_RIGHT,                                  \
    .Reference       = ADC_REFERENCE_VDD_VSS_8CH,                         \
    .ConvCpltCallback = NULL,                                             \
}

/* init / deinit. */

/**
 * @brief  Initialize the A/D converter with the given handle.
 *         Programs ADCON0/ADCON1 (clock, format, reference) and
 *         installs the conversion-complete callback.
 * @param h handle with Channel, ClockSource, ResultFormat, Reference.
 * @return EPIC_OK on success, EPIC_ERROR if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_ADC_Init(const ADC_HandleTypeDef *h);

/**
 * @brief  De-initialize the A/D converter. Clears the callback and
 *         returns ADCON0/ADCON1 to their reset state.
 * @return EPIC_OK on success.
 */
EPIC_StatusTypeDef EPIC_ADC_DeInit(void);

/* conversion control. */

/**
 * @brief  Select the analog channel and start a conversion.
 *         Sets ADCON0<GO/DONE> = 1. The user is expected to:
 *           1. Select the channel (with EPIC_ADC_SelectChannel)
 *           2. Wait for the acquisition time (§11.1, ~20 µs at 5 V)
 *           3. Call EPIC_ADC_Start() to begin conversion
 *           4. Poll EPIC_ADC_IsConversionDone() or wait for the IRQ
 *
 *         Returns 0xFFFF if a conversion was already in progress
 *         (GO/DONE was 1).
 * @return 0xFFFF if a conversion was already running, else the raw
 *         ADRESH/ADRESL pair (low byte undefined until conversion ends).
 */
uint16_t EPIC_ADC_Start(void);

/**
 * @brief Select the channel without starting conversion.
 * @param ch the analog channel to select.
 */
void EPIC_ADC_SelectChannel(ADC_ChannelTypeDef ch);

/**
 * @brief Returns 1 if a conversion is in progress (GO/DONE = 1).
 * @return 1 if a conversion is in progress, 0 otherwise.
 */
uint8_t EPIC_ADC_IsConversionInProgress(void);

/**
 * @brief Returns 1 if the latest conversion has completed (ADIF = 1).
 * @return 1 if the conversion has completed, 0 otherwise.
 */
uint8_t EPIC_ADC_IsConversionDone(void);

/**
 * @brief Clear the ADIF flag. Must be called in the conversion-complete IRQ.
 */
void EPIC_ADC_ClearITFlag(void);

/* result. */

/**
 * @brief  Read the latest 10-bit result. Returns 0..1023 in right-
 *         justified format; left-justified results are shifted down
 *         to 0..1023.
 * @return the latest 10-bit conversion result, 0..1023.
 */
uint16_t EPIC_ADC_Read(void);

/* interrupts. */

/**
 * @brief Weak ADC ISR, override in user code.
 */
void ADC_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_ADC_H */
