/*
 * ECCP1 (Enhanced CCP) + CCP2 driver (DS39632E §16.0): capture/compare/
 * PWM. CCP1 is the Enhanced CCP, adding multi-output PWM, programmable
 * dead-band (ECCP1DEL) and auto-shutdown/restart (ECCP1AS); CCP2 is plain.
 * Capture/Compare use Timer1 or Timer3 (T3CON<T3CCP2:T3CCP1>, reset
 * default Timer1); PWM always uses Timer2.
 */

#ifndef PIC18FXX5X_CCP_H
#define PIC18FXX5X_CCP_H

#include "pic18fxx5x.h"
#include "pic18fxx5x_sfr.h"

/**
 * @brief Which CCP module a handle refers to.
 */
typedef enum {
    CCP_INSTANCE_1 = 1,    /**< CCP1 / ECCP1, pins P1A (RC2 on 40/44-pin). */
    CCP_INSTANCE_2 = 2,    /**< CCP2, pin RC1/CCP2. */
} CCP_InstanceTypeDef;

/**
 * @brief CCP/ECCP operating mode (CCPxCON<3:0>, DS39632E Register 16-1).
 *        PIC18 adds CCP_MODE_COMPARE_TOGGLE (0010) vs. PIC16.
 */
typedef enum {
    CCP_MODE_OFF              = 0x0U,   /**< 0000, module disabled. */
    CCP_MODE_CAPTURE_FALLING  = 0x4U,   /**< 0100, every falling edge.  */
    CCP_MODE_CAPTURE_RISING   = 0x5U,   /**< 0101, every rising edge.   */
    CCP_MODE_CAPTURE_4TH      = 0x6U,   /**< 0110, every 4th rising.    */
    CCP_MODE_CAPTURE_16TH     = 0x7U,   /**< 0111, every 16th rising.   */
    CCP_MODE_COMPARE_TOGGLE   = 0x2U,   /**< 0010, toggle output on match (PIC18). */
    CCP_MODE_COMPARE_SET      = 0x8U,   /**< 1000, set output on match. */
    CCP_MODE_COMPARE_CLEAR    = 0x9U,   /**< 1001, clear output on match. */
    CCP_MODE_COMPARE_SOFT_IF  = 0xAU,   /**< 1010, software interrupt only. */
    CCP_MODE_COMPARE_TRIGGER  = 0xBU,   /**< 1011, special event trigger. */
    CCP_MODE_PWM              = 0xCU,   /**< 11xx, PWM (any 11xx). */
} CCP_ModeTypeDef;

/**
 * @brief Enhanced PWM output configuration (CCP1CON<P1M1:P1M0>, ECCP1 only,
 *        DS39632E Register 16-1). Ignored for CCP2 (plain, single output).
 */
typedef enum {
    CCP_PWM_OUTPUT_SINGLE       = 0x0U,  /**< 00: P1A modulated; P1B/C/D port pins. */
    CCP_PWM_OUTPUT_FULL_FORWARD = 0x1U,  /**< 01: full-bridge forward.            */
    CCP_PWM_OUTPUT_HALF_BRIDGE  = 0x2U,  /**< 10: P1A,P1B modulated w/ dead-band.  */
    CCP_PWM_OUTPUT_FULL_REVERSE = 0x3U,  /**< 11: full-bridge reverse.            */
} CCP_PWMOutputTypeDef;

/**
 * @brief Configuration for PWM mode. Duty is 10-bit, encoded as
 *        `duty_full = (CCPRxL:CCPxCON<5:4>)`. `Period` is the Timer2 PR2
 *        value (informational; program PR2 via the Timer2 driver).
 */
typedef struct {
    uint16_t Period;   /**< Timer2 PR2 value, 0..255. */
    uint16_t Duty;     /**< 10-bit PWM duty, 0..1023. */
} CCP_PWMConfigTypeDef;

/**
 * @brief Dead-band delay + auto-restart (ECCP1DEL, ECCP1 only).
 *        Relevant in half-bridge and full-bridge PWM modes. `Delay` is the
 *        7-bit PDC6:PDC0 count (dead-band time in instruction-cycle units
 *        per DS39632E §16.4.6). `AutoRestart` maps to PRSEN: when true, the
 *        PWM auto-restarts after an auto-shutdown clears; when false,
 *        firmware must clear ECCPASE to restart.
 */
typedef struct {
    uint8_t Delay;        /**< PDC6:PDC0, 0..127. */
    bool    AutoRestart;  /**< PRSEN. */
} CCP_DeadBandConfigTypeDef;

/**
 * @brief Auto-shutdown pin state on a shutdown event (PSSAC/PSSBD,
 *        DS39632E Register 16-3). 1x = tri-state.
 */
typedef enum {
    CCP_SHUTDOWN_DRIVE_0  = 0x0U,  /**< 00: drive the pin pair to 0. */
    CCP_SHUTDOWN_DRIVE_1  = 0x1U,  /**< 01: drive the pin pair to 1. */
    CCP_SHUTDOWN_TRISTATE = 0x2U,  /**< 1x: tri-state the pin pair. */
} CCP_PinStateTypeDef;

/**
 * @brief Auto-shutdown source (ECCPAS2:ECCPAS0, DS39632E Register 16-3).
 *        FLT0 is the external fault pin; the comparators are the analog
 *        comparators. `CCP_AUTOSHUTDOWN_DISABLED` turns auto-shutdown off.
 */
typedef enum {
    CCP_AUTOSHUTDOWN_DISABLED            = 0x0U,  /**< 000. */
    CCP_AUTOSHUTDOWN_COMP1               = 0x1U,  /**< 001: Comparator 1 output. */
    CCP_AUTOSHUTDOWN_COMP2               = 0x2U,  /**< 010: Comparator 2 output. */
    CCP_AUTOSHUTDOWN_COMP1_OR_COMP2      = 0x3U,  /**< 011. */
    CCP_AUTOSHUTDOWN_FLT0                = 0x4U,  /**< 100: FLT0 pin. */
    CCP_AUTOSHUTDOWN_FLT0_OR_COMP1       = 0x5U,  /**< 101. */
    CCP_AUTOSHUTDOWN_FLT0_OR_COMP2       = 0x6U,  /**< 110. */
    CCP_AUTOSHUTDOWN_FLT0_OR_COMP1_OR_COMP2 = 0x7U, /**< 111. */
} CCP_AutoShutdownSourceTypeDef;

/**
 * @brief Auto-shutdown configuration (ECCP1AS, ECCP1 only).
 */
typedef struct {
    CCP_AutoShutdownSourceTypeDef Source;  /**< ECCPAS2:ECCPAS0. */
    CCP_PinStateTypeDef           PinsAC;  /**< P1A/P1C shutdown state. */
    CCP_PinStateTypeDef           PinsBD;  /**< P1B/P1D shutdown state (40/44-pin). */
} CCP_AutoShutdownConfigTypeDef;

/**
 * @brief Driver handle (Cube-style). ECCP-only fields (PWMOutputMode,
 *        DeadBand, AutoShutdown) apply to CCP1; CCP2 ignores them.
 */
typedef struct {
    CCP_InstanceTypeDef            Instance;
    CCP_ModeTypeDef                Mode;
    uint16_t                       CompareValue;  /**< 16-bit compare/capture value. */
    CCP_PWMConfigTypeDef           PWM;           /**< PWM period + duty (PWM mode). */
    CCP_PWMOutputTypeDef           PWMOutputMode; /**< P1M (ECCP1 only). */
    CCP_DeadBandConfigTypeDef      DeadBand;      /**< ECCP1DEL (ECCP1 only). */
    CCP_AutoShutdownConfigTypeDef  AutoShutdown;  /**< ECCP1AS (ECCP1 only). */
    void (*EventCallback)(void);                  /**< Fires on CCP1IF / CCP2IF. */
} CCP_HandleTypeDef;

/**
 * @brief  Configure the CCP/ECCP module. Programs CCPxCON (mode + P1M for
 *         ECCP1 + duty LSBs), CCPRxL/H, ECCP1DEL (dead-band) and ECCP1AS
 *         (auto-shutdown) for ECCP1, and enables the matching interrupt if
 *         `EventCallback != NULL`.
 *
 * @note   For PWM, also call `EPIC_TIMER2_Init` + `EPIC_TIMER2_Start` with a
 *         period matching `h->PWM.Period` before/after this call (Timer2 is
 *         the PWM time base). For capture/compare, start Timer1 (or Timer3).
 * @param h the CCP handle describing the desired configuration.
 * @return 0 on success, 0xFFFF on invalid configuration.
 */
EPIC_StatusTypeDef EPIC_CCP_Init(const CCP_HandleTypeDef *h);

/**
 * @brief Reset CCPxCON (and ECCP1DEL/ECCP1AS for ECCP1) to 0x00; clear the
 *        PIR flag.
 * @param inst which CCP module to reset (CCP_INSTANCE_1 or CCP_INSTANCE_2).
 * @return 0 on success, 0xFFFF on invalid instance.
 */
EPIC_StatusTypeDef EPIC_CCP_DeInit(CCP_InstanceTypeDef inst);

/**
 * @brief Set the 16-bit CCPRx value (high byte first, DS39632E §16.x idiom).
 * @param inst which CCP module to configure.
 * @param value the 16-bit compare/capture value to load.
 */
void EPIC_CCP_SetCompare(CCP_InstanceTypeDef inst, uint16_t value);

/**
 * @brief Change only CCPxCON's mode field, leaving CCPRx and IRQ enable
 *        state untouched. Cheap: no flag-clear or IRQ-enable bookkeeping,
 *        unlike EPIC_CCP_Init. For repeated in-frame mode switches
 *        (bit-banged protocols reprogramming the module every bit), not
 *        one-time setup.
 * @param inst which CCP module to configure.
 * @param mode the new operating mode (a @ref CCP_ModeTypeDef value).
 */
void EPIC_CCP_SetMode(CCP_InstanceTypeDef inst, CCP_ModeTypeDef mode);

/**
 * @brief Atomically read the 16-bit CCPRx value (high-low-high idiom).
 * @param inst which CCP module to read.
 * @return the current 16-bit CCPRx value.
 */
uint16_t EPIC_CCP_GetCapture(CCP_InstanceTypeDef inst);

/**
 * @brief  Set PWM duty in 10-bit units (0..1023). Writes the LSBs into
 *         CCPxCON<5:4> then CCPRxL (bits 9:2), preserving the mode bits.
 * @param inst which CCP module to configure.
 * @param duty the 10-bit duty value, 0..1023.
 */
void EPIC_CCP_SetPWMDuty(CCP_InstanceTypeDef inst, uint16_t duty);

/**
 * @brief  Configure the ECCP1 dead-band delay + auto-restart (ECCP1DEL).
 *         No-op for CCP2. Relevant in half-bridge / full-bridge PWM modes.
 * @param inst which CCP module to configure (only CCP1 applies).
 * @param delay the 7-bit dead-band count, 0..127.
 * @param auto_restart true to auto-restart PWM after auto-shutdown (PRSEN).
 */
void EPIC_CCP_ConfigDeadBand(CCP_InstanceTypeDef inst,
                            uint8_t delay, bool auto_restart);

/**
 * @brief  Configure the ECCP1 auto-shutdown source + pin states (ECCP1AS).
 *         No-op for CCP2. Pass `CCP_AUTOSHUTDOWN_DISABLED` to turn it off.
 * @param inst which CCP module to configure (only CCP1 applies).
 * @param source the auto-shutdown source (a @ref CCP_AutoShutdownSourceTypeDef value).
 * @param pins_ac the P1A/P1C shutdown pin state.
 * @param pins_bd the P1B/P1D shutdown pin state.
 */
void EPIC_CCP_ConfigAutoShutdown(CCP_InstanceTypeDef inst,
                                CCP_AutoShutdownSourceTypeDef source,
                                CCP_PinStateTypeDef pins_ac,
                                CCP_PinStateTypeDef pins_bd);

/**
 * @brief Returns 1 if an ECCP1 auto-shutdown event is active
 *        (ECCP1AS<ECCPASE>).
 * @param inst which CCP module to query (only CCP1 applies).
 * @return 1 while an auto-shutdown event is active, else 0.
 */
uint8_t EPIC_CCP_IsShutdown(CCP_InstanceTypeDef inst);

/**
 * @brief  Clear the ECCP1 auto-shutdown status (ECCPASE) to restart the PWM.
 *         Only effective when PRSEN = 0 (manual restart); with PRSEN = 1 the
 *         hardware auto-clears when the shutdown source deasserts. No-op for
 *         CCP2.
 * @param inst which CCP module to restart (only CCP1 applies).
 */
void EPIC_CCP_Restart(CCP_InstanceTypeDef inst);

/**
 * @brief Weak CCP1 (ECCP1) ISR, override in user code.
 */
void CCP1_IRQHandler(void) EPIC_WEAK;
/**
 * @brief Weak CCP2 ISR, override in user code.
 */
void CCP2_IRQHandler(void) EPIC_WEAK;

#endif /* PIC18FXX5X_CCP_H */
