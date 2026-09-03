/* CCP1 / CCP2 driver, Capture / Compare / PWM. Source: DS39582B §8.0;
 * full register reference: MANUAL.md §13. CCP1's special-event trigger
 * resets Timer1; CCP2's also starts an A/D conversion (§8.2.4).
 * Capture/Compare need Timer1 running; PWM needs Timer2. */

#ifndef PIC16F87XA_CCP_H
#define PIC16F87XA_CCP_H

#include "pic16f87xa.h"
#include "pic16f87xa_sfr.h"

/**
 * @brief Which CCP module a handle refers to.
 */
typedef enum {
    CCP_INSTANCE_1 = 1,    /**< CCP1, pins RC2/CCP1. */
    CCP_INSTANCE_2 = 2,    /**< CCP2, pins RC1/CCP2. */
} CCP_InstanceTypeDef;

/**
 * @brief CCP operating mode (CCPxCON<3:0>, DS39582B Register 8-1).
 */
typedef enum {
    CCP_MODE_OFF              = 0x0U,   /**< 0000, module disabled. */
    CCP_MODE_COMPARE_SET      = 0x8U,   /**< 1000, set output on match. */
    CCP_MODE_COMPARE_CLEAR    = 0x9U,   /**< 1001, clear output on match. */
    CCP_MODE_COMPARE_SOFT_IF  = 0xAU,   /**< 1010, software interrupt only. */
    CCP_MODE_COMPARE_TRIGGER  = 0xBU,   /**< 1011, special event trigger. */
    CCP_MODE_PWM              = 0xCU,   /**< 11xx, PWM (any 11xx). */
    CCP_MODE_CAPTURE_FALLING  = 0x4U,   /**< 0100, every falling edge.  */
    CCP_MODE_CAPTURE_RISING   = 0x5U,   /**< 0101, every rising edge.   */
    CCP_MODE_CAPTURE_4TH      = 0x6U,   /**< 0110, every 4th rising.    */
    CCP_MODE_CAPTURE_16TH     = 0x7U,   /**< 0111, every 16th rising.   */
} CCP_ModeTypeDef;

/**
 * @brief Configuration for PWM mode.
 *
 * Duty is 10-bit, encoded as `duty_full = (CCPRxL:CCPxCON<5:4>)`.
 * Set `Duty` in the range 0..1023; the driver writes it into
 * CCPRxL and CCPxCON<5:4>.
 */
typedef struct {
    uint16_t Period;   /**< Timer2 PR2 value, 0..255. */
    uint16_t Duty;     /**< 10-bit PWM duty, 0..1023. */
} CCP_PWMConfigTypeDef;

/**
 * @brief Driver handle (Cube-style).
 *
 *   One EPIC_CCP_HandleTypeDef is sufficient per CCP instance; the same
 *   struct can also be reused for both.
 */
typedef struct {
    CCP_InstanceTypeDef      Instance;
    CCP_ModeTypeDef          Mode;
    /** @brief 16-bit compare / capture value. For capture mode this is
     *         the last captured value (read-only). For compare / PWM it
     *         is the value to match / the duty cycle. */
    uint16_t                 CompareValue;
    /** @brief PWM configuration (only used when Mode == CCP_MODE_PWM). */
    CCP_PWMConfigTypeDef     PWM;
    /** @brief Optional event callback (fires on CCP1IF / CCP2IF). */
    void (*EventCallback)(void);
} CCP_HandleTypeDef;

/* init / deinit. */

/**
 * @brief  Configure the CCP module. Programs CCPxCON, sets the initial
 *         compare/capture value, and (if `EventCallback != NULL`)
 *         enables the matching PIR1/PIR2 interrupt.
 *
 * @param  h     handle with Instance, Mode, CompareValue, optional PWM.
 *
 * @note   The EventCallback is copied into driver-owned storage: the
 *         IRQ handlers call the driver's copy directly, so `h` does
 *         not need to outlive this call, and the callback keeps
 *         firing until EPIC_CCP_DeInit clears it.
 *
 * @note   For PWM, also call `EPIC_TIMER2_Init` + `EPIC_TIMER2_Start`
 *         with a period matching `h->PWM.Period` before this call.
 *
 * @note   For capture, also start Timer1 manually.
 * @return EPIC_OK on success, EPIC_ERROR on invalid handle or instance.
 */
EPIC_StatusTypeDef EPIC_CCP_Init(const CCP_HandleTypeDef *h);

/**
 * @brief Reset CCPxCON to 0x00 and clear the corresponding PIR flag.
 * @param inst which CCP module (CCP_INSTANCE_1 or CCP_INSTANCE_2).
 * @return EPIC_OK on success, EPIC_ERROR on invalid instance.
 */
EPIC_StatusTypeDef EPIC_CCP_DeInit(CCP_InstanceTypeDef inst);

/* compare / capture / pwm. */

/**
 * @brief Set the 16-bit CCPRx value.
 * @param inst which CCP module to program.
 * @param value the 16-bit compare/capture value to write.
 */
void EPIC_CCP_SetCompare(CCP_InstanceTypeDef inst, uint16_t value);

/**
 * @brief Change only CCPxCON's mode field, leaving CCPRx and IRQ enable state
 *  untouched. Cheap: no flag-clear or IRQ-enable bookkeeping, unlike
 *  EPIC_CCP_Init. For repeated in-frame mode switches (bit-banged
 *  protocols reprogramming the module every bit), not one-time setup.
 *  No-op if `inst` is not a valid CCP instance.
 * @param inst which CCP module to reprogram.
 * @param mode the new CCP mode (see @ref CCP_ModeTypeDef).
 */
void EPIC_CCP_SetMode(CCP_InstanceTypeDef inst, CCP_ModeTypeDef mode);

/**
 * @brief Atomically read the 16-bit CCPRx value.
 * @param inst which CCP module to read.
 * @return the current 16-bit CCPRx value.
 */
uint16_t EPIC_CCP_GetCapture(CCP_InstanceTypeDef inst);

/**
 * @brief  Set PWM duty in 10-bit units (0..1023).
 *         For duty=0 the output stays low for the entire period.
 *         For duty > period the output stays high (per §8.3.2 Note).
 * @param inst which CCP module to configure.
 * @param duty the 10-bit duty value, 0..1023.
 */
void EPIC_CCP_SetPWMDuty(CCP_InstanceTypeDef inst, uint16_t duty);

/* IRQ entries. */

/**
 * @brief Weak CCP1 ISR, override in user code.
 */
void CCP1_IRQHandler(void) EPIC_WEAK;
/**
 * @brief Weak CCP2 ISR, override in user code.
 */
void CCP2_IRQHandler(void) EPIC_WEAK;

#endif /* PIC16F87XA_CCP_H */
