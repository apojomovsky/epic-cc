/* Shared PIC14 mid-range CCP driver (87XA + 88X ECCP1 variant + 628A
 * CCP1-only). Sources: DS39582B §8 (87XA), DS40001291H §11 (88X); full
 * reference: the family MANUAL.md. CCP1 is the Enhanced CCP on ECCP
 * parts: PWM output config (single/half/full bridge, P1M<1:0>),
 * dead-time control (PWM1CON), auto-shutdown (ECCPAS) and pulse
 * steering (PSTRCON); elsewhere it is the plain CCP. Capture/Compare
 * need Timer1 running; PWM needs Timer2. Parts without CCP2 (628A)
 * run the same file with an empty instance-2. */

#ifndef PIC14_CCP_H
#define PIC14_CCP_H

#include "pic14_midrange.h"
#include "pic14_midrange_sfr.h"

/**
 * @brief Which CCP module a handle refers to.
 */
typedef enum {
    CCP_INSTANCE_1 = 1,    /**< ECCP1, pins P1A..P1D (RC2/P1A, RC1/CCP2). */
#if PIC14MIDRANGE_HAS_CCP2
    CCP_INSTANCE_2 = 2,    /**< CCP2, pins RC1/CCP2. */
#endif
} CCP_InstanceTypeDef;

/**
 * @brief CCP operating mode (CCPxCON<3:0>, DS40001291H Register 11-1).
 */
typedef enum {
    CCP_MODE_OFF              = 0x0U,   /**< 0000, module disabled. */
    CCP_MODE_COMPARE_TOGGLE   = 0x2U,   /**< 0010, toggle output on match. */
    CCP_MODE_CAPTURE_FALLING  = 0x4U,   /**< 0100, every falling edge.  */
    CCP_MODE_CAPTURE_RISING   = 0x5U,   /**< 0101, every rising edge.   */
    CCP_MODE_CAPTURE_4TH      = 0x6U,   /**< 0110, every 4th rising.    */
    CCP_MODE_CAPTURE_16TH     = 0x7U,   /**< 0111, every 16th rising.   */
    CCP_MODE_COMPARE_SET      = 0x8U,   /**< 1000, set output on match. */
    CCP_MODE_COMPARE_CLEAR    = 0x9U,   /**< 1001, clear output on match. */
    CCP_MODE_COMPARE_SOFT_IF  = 0xAU,   /**< 1010, software interrupt only. */
    CCP_MODE_COMPARE_TRIGGER  = 0xBU,   /**< 1011, special event trigger. */
    CCP_MODE_PWM              = 0xCU,   /**< 11xx, PWM (any 11xx). */
} CCP_ModeTypeDef;

#if PIC14MIDRANGE_HAS_ECCP
/**
 * @brief ECCP PWM output configuration (CCP1CON<P1M1:P1M0>, Register
 *        11-1). Only meaningful for CCP1 in PWM mode.
 */
typedef enum {
    CCP_PWM_SINGLE     = 0x0U,   /**< 00: P1A modulated; P1B/C/D port pins. */
    CCP_PWM_FULL_FWD   = 0x1U,   /**< 01: full-bridge forward; P1D modulated. */
    CCP_PWM_HALF       = 0x2U,   /**< 10: half-bridge; P1A/P1B with dead-band. */
    CCP_PWM_FULL_REV   = 0x3U,   /**< 11: full-bridge reverse; P1B modulated. */
} CCP_PWMOutputTypeDef;

/**
 * @brief ECCP auto-shutdown source (ECCPAS<ECCPAS2:ECCPAS0>, Register
 *        11-3).
 */
typedef enum {
    CCP_AS_DISABLED         = 0x0U,   /**< 000: auto-shutdown disabled. */
    CCP_AS_C1_HIGH          = 0x1U,   /**< 001: comparator C1 output high. */
    CCP_AS_C2_HIGH          = 0x2U,   /**< 010: comparator C2 output high. */
    CCP_AS_ANY_COMP_HIGH    = 0x3U,   /**< 011: either comparator output high. */
    CCP_AS_INT_LOW          = 0x4U,   /**< 100: VIL on INT pin. */
    CCP_AS_INT_OR_C1        = 0x5U,   /**< 101: INT low or C1 high. */
    CCP_AS_INT_OR_C2        = 0x6U,   /**< 110: INT low or C2 high. */
    CCP_AS_INT_OR_ANY_COMP  = 0x7U,   /**< 111: INT low or either comparator high. */
} CCP_AutoShutdownSourceTypeDef;

/**
 * @brief ECCP auto-shutdown output state (ECCPAS<PSSAC1:PSSAC0> /
 *        <PSSBD1:PSSBD0>, Register 11-3).
 */
typedef enum {
    CCP_AS_STATE_LOW   = 0x0U,   /**< 00: drive pins to 0. */
    CCP_AS_STATE_HIGH  = 0x1U,   /**< 01: drive pins to 1. */
    CCP_AS_STATE_TRISTATE = 0x2U, /**< 1x: pins tri-state. */
} CCP_AutoShutdownStateTypeDef;

/**
 * @brief Pulse-steering pin mask (PSTRCON<STRD:STRA>, Register 11-5).
 *        Only in Single-output PWM mode; OR these together.
 */
#define CCP_STEER_P1A  PIC_PSTRCON_STRA
#define CCP_STEER_P1B  PIC_PSTRCON_STRB
#define CCP_STEER_P1C  PIC_PSTRCON_STRC
#define CCP_STEER_P1D  PIC_PSTRCON_STRD
#endif

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
#if PIC14MIDRANGE_HAS_ECCP
    /** @brief ECCP1-only PWM output config (P1M<1:0>). */
    CCP_PWMOutputTypeDef     PWMOutput;
#endif
    /** @brief Optional event callback (fires on CCP1IF / CCP2IF). */
    void (*EventCallback)(void);
    /** @brief 16-bit compare / capture value. For capture mode this is
     *         the last captured value (read-only). For compare it is
     *         the value to match. */
    uint16_t                 CompareValue;
    /** @brief PWM configuration (only used when Mode == CCP_MODE_PWM). */
    CCP_PWMConfigTypeDef     PWM;
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
 * @brief Reset CCPxCON to 0x00, clear the corresponding PIR flag and,
 *        for ECCP1, reset the PWM1CON/ECCPAS/PSTRCON enhancement
 *        registers.
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
 *         For duty > period the output stays high (per §11.3.2 Note).
 * @param inst which CCP module to configure.
 * @param duty the 10-bit duty value, 0..1023.
 */
void EPIC_CCP_SetPWMDuty(CCP_InstanceTypeDef inst, uint16_t duty);

#if PIC14MIDRANGE_HAS_ECCP
/* ECCP1-only enhanced-PWM helpers (no-op on CCP2). */

/**
 * @brief  Configure the ECCP1 PWM output mode (P1M<1:0>) and the
 *         auto-shutdown source / output states (ECCPAS), and the
 *         dead-time / restart bits (PWM1CON).
 *
 * @param  pwm_out      CCP_PWM_SINGLE / HALF / FULL_FWD / FULL_REV.
 * @param  as_source    auto-shutdown source (CCP_AS_*), 0 disables.
 * @param  as_state_ac  shutdown state for P1A/P1C.
 * @param  as_state_bd  shutdown state for P1B/P1D.
 * @param  deadtime     7-bit dead-time delay count (PDC<6:0>), in
 *                      FOSC/4 cycles. Must stay below the duty cycle
 *                      (silicon errata DS80000302K item 12).
 * @param  auto_restart 1 = PRSEN (PWM restarts when the shutdown event
 *                      clears), 0 = ECCPASE must be cleared in software.
 *
 * @note   All three registers (PWM1CON/ECCPAS/PSTRCON) live in Bank 1.
 */
void EPIC_CCP1_ConfigEnhanced(uint8_t pwm_out,
                              uint8_t as_source,
                              uint8_t as_state_ac,
                              uint8_t as_state_bd,
                              uint8_t deadtime,
                              uint8_t auto_restart);

/**
 * @brief  Set the ECCP1 pulse-steering mask (PSTRCON<STRD:STRA>):
 *         which P1 pins carry the PWM waveform in Single-output mode.
 *         OR the @ref CCP_STEER_P1x macros together.
 * @param  steer_mask the steering bitmask.
 * @param  sync 1 = steering updates on the next PWM period (STRSYNC),
 *              0 = at the instruction-cycle boundary.
 */
void EPIC_CCP1_SetSteering(uint8_t steer_mask, uint8_t sync);

/**
 * @brief  Report whether an auto-shutdown event is active (ECCPASE).
 * @return 1 if ECCP outputs are in the shutdown state, 0 otherwise.
 */
uint8_t EPIC_CCP1_IsShutdown(void);

/**
 * @brief  Clear the auto-shutdown latch (ECCPASE) to restart the PWM
 *         when PRSEN = 0. Writing is ignored while the shutdown
 *         condition persists (DS40001291H Register 11-3 note 2).
 */
void EPIC_CCP1_ClearShutdown(void);
#endif

/* IRQ entries. */

/**
 * @brief Weak CCP1 ISR, override in user code.
 */
void CCP1_IRQHandler(void) EPIC_WEAK;
#if PIC14MIDRANGE_HAS_CCP2
/**
 * @brief Weak CCP2 ISR, override in user code.
 */
void CCP2_IRQHandler(void) EPIC_WEAK;
#endif

#endif /* PIC14_CCP_H */
