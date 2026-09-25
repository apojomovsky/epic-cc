/*
 * A/D converter driver, implementation (DS39632E §21.0). No bank
 * switching (all ADC registers are in the Access Bank), but three control
 * registers (ADCON0/1/2) versus PIC16's two. The sim backend models
 * conversion completion via `pic18_sim_drive_adc_done()`.
 */

#include "peripherals/pic18fxx5x_adc.h"
#include "core/pic18_irq.h"

static ADC_HandleTypeDef        g_adc_storage;
static const ADC_HandleTypeDef *g_adc = NULL;

/**
 * @brief  Initialize the A/D converter from a handle.
 *
 *         Programs ADCON0/1/2 (channel, voltage reference, pin config,
 *         result format, acquisition time, clock source) and arms the
 *         conversion-complete interrupt if a callback is provided.
 * @param h Handle describing the ADC configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL.
 */
EPIC_StatusTypeDef EPIC_ADC_Init(const ADC_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    g_adc_storage = *h;
    g_adc = &g_adc_storage;

    /* ADCON0 (Register 21-1): ADON | CHS3:CHS0. GO/DONE cleared. */
    uint8_t adcon0 = PIC_ADCON0_ADON;
    adcon0 |= (uint8_t)((h->Channel & 0x0FU) << PIC_ADCON0_CHS_POS);
    epic_sfr_write8(PIC_REG_ADCON0, adcon0);

    /* ADCON1 (Register 21-2): VCFG1:VCFG0 | PCFG3:PCFG0. */
    uint8_t adcon1 = (uint8_t)(h->VReference & (PIC_ADCON1_VCFG0 | PIC_ADCON1_VCFG1));
    adcon1 |= (uint8_t)(h->PinConfig & PIC_ADCON1_PCFG_MASK);
    epic_sfr_write8(PIC_REG_ADCON1, adcon1);

    /* ADCON2 (Register 21-3): ADFM | ACQT2:ACQT0 | ADCS2:ADCS0. */
    uint8_t adcon2 = 0U;
    if (h->ResultFormat == ADC_FORMAT_RIGHT) adcon2 |= PIC_ADCON2_ADFM;
    adcon2 |= (uint8_t)((h->Acquisition & 0x07U) << PIC_ADCON2_ACQT_POS);
    adcon2 |= (uint8_t)(h->ClockSource & PIC_ADCON2_ADCS_MASK);
    epic_sfr_write8(PIC_REG_ADCON2, adcon2);

    /* Interrupt enable. */
    EPIC_IRQ_ClearFlag(PIC18_IRQ_ADC);
    if (h->ConvCpltCallback) EPIC_IRQ_Enable(PIC18_IRQ_ADC);
    else                     EPIC_IRQ_DisableSrc(PIC18_IRQ_ADC);

    return EPIC_OK;
}

/**
 * @brief  De-initialize the A/D converter: disable its interrupt, clear
 *         the flag, restore ADCON0/1/2 to their power-on values and drop
 *         the stored handle.
 * @return EPIC_OK.
 */
EPIC_StatusTypeDef EPIC_ADC_DeInit(void)
{
    EPIC_IRQ_DisableSrc(PIC18_IRQ_ADC);
    EPIC_IRQ_ClearFlag(PIC18_IRQ_ADC);
    epic_sfr_write8(PIC_REG_ADCON0, PIC_ADCON0_POR_VALUE);
    epic_sfr_write8(PIC_REG_ADCON1, PIC_ADCON1_POR_VALUE);
    epic_sfr_write8(PIC_REG_ADCON2, PIC_ADCON2_POR_VALUE);
    g_adc = NULL;
    return EPIC_OK;
}

/**
 * @brief  Select the analog input channel without starting a conversion.
 * @param ch Channel number (0..13, 4-bit CHS field).
 */
void EPIC_ADC_SelectChannel(ADC_ChannelTypeDef ch)
{
    uint8_t v = epic_sfr_read8(PIC_REG_ADCON0);
    v = (uint8_t)((v & (uint8_t)~PIC_ADCON0_CHS_MASK) |
                  ((uint8_t)(ch & 0x0FU) << PIC_ADCON0_CHS_POS));
    epic_sfr_write8(PIC_REG_ADCON0, v);
}

/**
 * @brief  Start a conversion by setting ADCON0<GO/DONE>. The caller is
 *         expected to select the channel, wait the acquisition time,
 *         call Start, then poll IsConversionDone() or wait for the IRQ.
 * @return 0 on success, 0xFFFF if a conversion was already in progress.
 */
uint16_t EPIC_ADC_Start(void)
{
    uint8_t v = epic_sfr_read8(PIC_REG_ADCON0);
    if (v & PIC_ADCON0_GO_DONE) return 0xFFFFU;     /* already in progress */
    epic_sfr_write8(PIC_REG_ADCON0, (uint8_t)(v | PIC_ADCON0_GO_DONE));
    return 0U;
}

/**
 * @brief  Return 1 if a conversion is in progress (GO/DONE = 1).
 * @return 1 if converting, else 0.
 */
uint8_t EPIC_ADC_IsConversionInProgress(void)
{
    return (epic_sfr_read8(PIC_REG_ADCON0) & PIC_ADCON0_GO_DONE) ? 1U : 0U;
}

/**
 * @brief  Return 1 if the latest conversion has completed (ADIF = 1).
 * @return 1 if complete, else 0.
 */
uint8_t EPIC_ADC_IsConversionDone(void)
{
    return (epic_sfr_read8(PIC_REG_PIR1) & PIC_PIR1_ADIF) ? 1U : 0U;
}

/**
 * @brief  Clear the ADIF flag; must be called in the conversion-complete
 *         IRQ handler.
 */
void EPIC_ADC_ClearITFlag(void)
{
    EPIC_IRQ_ClearFlag(PIC18_IRQ_ADC);
}

/**
 * @brief  Read the latest 10-bit conversion result (0..1023). Left-
 *         justified results (ADFM = 0) are shifted down so the caller
 *         always gets a 0..1023 value.
 * @return The 10-bit ADC result.
 */
uint16_t EPIC_ADC_Read(void)
{
    /* Read ADRESL then ADRESH. */
    uint8_t lo  = epic_sfr_read8(PIC_REG_ADRESL);
    uint8_t hi  = epic_sfr_read8(PIC_REG_ADRESH);
    uint16_t raw = (uint16_t)(((uint16_t)hi << 8) | lo);
    /* Right-shift if left-justified (ADFM=0) so the caller always gets a
     * 0..1023 result (DS39632E Register 21-3 + Figure 21-4). */
    uint8_t adfm = (uint8_t)(epic_sfr_read8(PIC_REG_ADCON2) & PIC_ADCON2_ADFM);
    if (!adfm) raw = (uint16_t)(raw >> 6);
    return (uint16_t)(raw & 0x03FFU);
}

/**
 * @brief  Weak ADC interrupt handler: clears ADIF and forwards the result
 *         to the conversion-complete callback registered via Init.
 */
void ADC_IRQHandler(void)
{
    if (!EPIC_IRQ_GetFlag(PIC18_IRQ_ADC)) return;
    EPIC_IRQ_ClearFlag(PIC18_IRQ_ADC);
    if (g_adc && g_adc->ConvCpltCallback) g_adc->ConvCpltCallback(EPIC_ADC_Read());
}
