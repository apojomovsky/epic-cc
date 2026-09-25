/*
 * ECCP1 (Enhanced CCP) + CCP2 driver, implementation (DS39632E §16.0).
 */

#include "peripherals/pic18fxx5x_ccp.h"
#include "core/pic18_irq.h"

/* Per-instance register access. Each macro branches on `inst` before
 * touching any SFR, so the address is always a literal `PIC_REG_*` token,
 * never a runtime value (see `pic18_irq.c`'s file header for why that
 * matters on PIC18). CCP1 is the enhanced module (ECCP1DEL/ECCP1AS);
 * CCP2 is plain. */
#define CCP_WRITE_CPRL(inst, value)                                     \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) EPIC_REG8(PIC_REG_CCPR1L) = (uint8_t)(value); \
        else                          EPIC_REG8(PIC_REG_CCPR2L) = (uint8_t)(value); \
    } while (0)
#define CCP_WRITE_CPRH(inst, value)                                     \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) EPIC_REG8(PIC_REG_CCPR1H) = (uint8_t)(value); \
        else                          EPIC_REG8(PIC_REG_CCPR2H) = (uint8_t)(value); \
    } while (0)
#define CCP_WRITE_CON(inst, value)                                      \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) EPIC_REG8(PIC_REG_CCP1CON) = (uint8_t)(value); \
        else                          EPIC_REG8(PIC_REG_CCP2CON) = (uint8_t)(value); \
    } while (0)
#define CCP_READ_CPRL(inst, out)                                        \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) (out) = EPIC_REG8(PIC_REG_CCPR1L); \
        else                          (out) = EPIC_REG8(PIC_REG_CCPR2L); \
    } while (0)
#define CCP_READ_CPRH(inst, out)                                        \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) (out) = EPIC_REG8(PIC_REG_CCPR1H); \
        else                          (out) = EPIC_REG8(PIC_REG_CCPR2H); \
    } while (0)
#define CCP_READ_CON(inst, out)                                         \
    do {                                                                \
        if ((inst) == CCP_INSTANCE_1) (out) = EPIC_REG8(PIC_REG_CCP1CON); \
        else                          (out) = EPIC_REG8(PIC_REG_CCP2CON); \
    } while (0)

/**
 * @brief  Map a CCP instance to its interrupt ID. The interrupt ID is not
 *         an SFR address (just a small enum passed into pic18_irq.c's own,
 *         already-fixed dispatch), so a plain lookup is fine here, nothing
 *         at risk.
 * @param inst CCP instance (CCP_INSTANCE_1 or CCP_INSTANCE_2).
 * @return PIC18_IRQ_CCP1 for instance 1, else PIC18_IRQ_CCP2.
 */
static PIC18_IRQn ccp_irq(CCP_InstanceTypeDef inst)
{
    return (inst == CCP_INSTANCE_1) ? PIC18_IRQ_CCP1 : PIC18_IRQ_CCP2;
}

/* Static handle storage, one per CCP instance. COPIES the caller's handle
 * (dangling-pointer rationale, see Timer1). The weak ISRs read from these
 * directly (EventCallback's own NULL-ness is the "is this instance
 * active" sentinel, g_ccp_storage is zero-initialized); there is no
 * separate pointer-array indirection to a runtime-indexed slot's
 * address, which epic-cc's isel-pic18 cannot yet materialize as a
 * value to store elsewhere (apojomovsky/epic-cc#468). */
static CCP_HandleTypeDef g_ccp_storage[3];

/**
 * @brief  Encode a PinState enum into the 2-bit PSS field value
 *         (0b00/01/10).
 * @param s Pin state to encode (DRIVE_0, DRIVE_1 or TRISTATE).
 * @return The 2-bit PSS encoding of `s`.
 */
static uint8_t pss_encode(CCP_PinStateTypeDef s)
{
    /* DRIVE_0 -> 00, DRIVE_1 -> 01, TRISTATE -> 10 (1x). */
    return (uint8_t)(s & 0x3U);
}

/**
 * @brief  Initialize a CCP module from a handle. Stores an owned copy of
 *         the handle, clears and optionally enables the instance IRQ, then
 *         programs the 16-bit CCPRx value and mode (PWM duty or
 *         capture/compare). ECCP1 additionally gets dead-band and
 *         auto-shutdown configuration.
 * @param h Handle describing the CCP instance and its configuration.
 * @return EPIC_OK on success, EPIC_INVALID if `h` is NULL or its Instance
 *         is not CCP_INSTANCE_1/2.
 */
EPIC_StatusTypeDef EPIC_CCP_Init(const CCP_HandleTypeDef *h)
{
    if (!h) return EPIC_INVALID;
    if (h->Instance != CCP_INSTANCE_1 && h->Instance != CCP_INSTANCE_2)
    {
        return EPIC_INVALID;
    }
    /* Field-by-field, not `g_ccp_storage[h->Instance] = *h;`: scalar
     * stores through a runtime-indexed global are fine, only
     * materializing a runtime-indexed GEP's address as a value (the
     * former g_ccp_handles[h->Instance] = &g_ccp_storage[h->Instance]
     * pointer-array pattern, removed) panics epic-cc's isel-pic18
     * (apojomovsky/epic-cc#468). */
    CCP_HandleTypeDef *dst = &g_ccp_storage[h->Instance];
    dst->Instance             = h->Instance;
    dst->Mode                 = h->Mode;
    dst->CompareValue         = h->CompareValue;
    dst->PWM.Period           = h->PWM.Period;
    dst->PWM.Duty             = h->PWM.Duty;
    dst->PWMOutputMode        = h->PWMOutputMode;
    dst->DeadBand.Delay       = h->DeadBand.Delay;
    dst->DeadBand.AutoRestart = h->DeadBand.AutoRestart;
    dst->AutoShutdown.Source  = h->AutoShutdown.Source;
    dst->AutoShutdown.PinsAC  = h->AutoShutdown.PinsAC;
    dst->AutoShutdown.PinsBD  = h->AutoShutdown.PinsBD;
    dst->EventCallback        = h->EventCallback;

    /* Clear/rearm the IRQ before reconfiguring. */
    EPIC_IRQ_ClearFlag(ccp_irq(h->Instance));
    if (h->EventCallback)
    {
        EPIC_IRQ_Enable(ccp_irq(h->Instance));
    }
    else
    {
        EPIC_IRQ_DisableSrc(ccp_irq(h->Instance));
    }

    if (h->Mode == CCP_MODE_PWM)
    {
        /* DS39632E §16.4.3 step 2: set the PWM duty BEFORE enabling PWM.
         * 10-bit duty: CCPRxL = duty[9:2], CCPxCON<5:4> = duty[1:0]. */
        uint16_t duty = (uint16_t)(h->PWM.Duty & 0x03FFU);
        uint8_t  con  = (uint8_t)(h->Mode & PIC_CCP1_M_MASK);   /* 11xx */
        con |= (uint8_t)((duty & 0x03U) << 4);                   /* duty[1:0] */
        if (h->Instance == CCP_INSTANCE_1)
        {
            con |= (uint8_t)((h->PWMOutputMode & 0x3U) << 6);   /* P1M[7:6] */
        }
        CCP_WRITE_CPRL(h->Instance, duty >> 2);
        CCP_WRITE_CPRH(h->Instance, 0U);
        CCP_WRITE_CON(h->Instance, con);
    }
    else
    {
        /* Capture / compare: write the 16-bit value then enable mode. */
        CCP_WRITE_CPRH(h->Instance, h->CompareValue >> 8);
        CCP_WRITE_CPRL(h->Instance, h->CompareValue & 0xFFU);
        uint8_t con = (uint8_t)(h->Mode & PIC_CCP1_M_MASK);
        if (h->Instance == CCP_INSTANCE_1)
        {
            con |= (uint8_t)((h->PWMOutputMode & 0x3U) << 6);
        }
        CCP_WRITE_CON(h->Instance, con);
    }

    /* ECCP1-only: dead-band + auto-restart (ECCP1DEL) and auto-shutdown
     * (ECCP1AS). CCP2 has neither; skip. */
    if (h->Instance == CCP_INSTANCE_1)
    {
        uint8_t del = (uint8_t)(h->DeadBand.Delay & PIC_ECCP1DEL_PDC_MASK);
        if (h->DeadBand.AutoRestart) del |= PIC_ECCP1DEL_PRSEN;
        EPIC_REG8(PIC_REG_ECCP1DEL) = del;
        /* ECCP1AS: source[6:4] | PSSAC[3:2] | PSSBD[1:0]; ECCPASE (bit 7)
         * is the status bit, left 0 here. */
        uint8_t asv = (uint8_t)(((h->AutoShutdown.Source & 0x7U) << 4) |
                                (pss_encode(h->AutoShutdown.PinsAC) << 2) |
                                pss_encode(h->AutoShutdown.PinsBD));
        EPIC_REG8(PIC_REG_ECCP1AS) = asv;
    }

    return EPIC_OK;
}

/**
 * @brief  Reset a CCP module: clear the PIR flag, disable the IRQ, reset
 *         CCPxCON (and ECCP1DEL/ECCP1AS for ECCP1) to 0x00 and drop the
 *         stored handle.
 * @param inst CCP instance to de-initialize.
 * @return EPIC_OK on success, EPIC_INVALID for an unknown instance.
 */
EPIC_StatusTypeDef EPIC_CCP_DeInit(CCP_InstanceTypeDef inst)
{
    if (inst != CCP_INSTANCE_1 && inst != CCP_INSTANCE_2) return EPIC_INVALID;
    EPIC_IRQ_DisableSrc(ccp_irq(inst));
    EPIC_IRQ_ClearFlag(ccp_irq(inst));
    CCP_WRITE_CON(inst, 0x00U);
    if (inst == CCP_INSTANCE_1)
    {
        EPIC_REG8(PIC_REG_ECCP1DEL) = PIC_ECCP1DEL_POR_VALUE;
        EPIC_REG8(PIC_REG_ECCP1AS) = PIC_ECCP1AS_POR_VALUE;
    }
    g_ccp_storage[inst].EventCallback = NULL;
    return EPIC_OK;
}

/**
 * @brief  Set the 16-bit CCPRx compare value, high byte first to avoid a
 *         spurious compare match (DS39632E §16.x). No-op for an unknown
 *         instance.
 * @param inst CCP instance.
 * @param value 16-bit compare value to load.
 */
void EPIC_CCP_SetCompare(CCP_InstanceTypeDef inst, uint16_t value)
{
    if (inst != CCP_INSTANCE_1 && inst != CCP_INSTANCE_2) return;
    /* High byte first to avoid a spurious compare match (DS39632E §16.x). */
    CCP_WRITE_CPRH(inst, value >> 8);
    CCP_WRITE_CPRL(inst, value & 0xFFU);
}

/**
 * @brief  Change only the CCPxCON mode field, leaving CCPRx and the IRQ
 *         enable state untouched. Cheap, so it suits repeated in-frame
 *         mode switches (bit-banged protocols reprogramming the module
 *         every bit). No-op for an unknown instance.
 * @param inst CCP instance.
 * @param mode New CCP mode to write into the mode field.
 */
void EPIC_CCP_SetMode(CCP_InstanceTypeDef inst, CCP_ModeTypeDef mode)
{
    if (inst != CCP_INSTANCE_1 && inst != CCP_INSTANCE_2) return;
    CCP_WRITE_CON(inst, (uint8_t)(mode & 0x0FU));
}

/**
 * @brief  Atomically read the 16-bit CCPRx capture value using the
 *         high-low-high idiom.
 * @param inst CCP instance.
 * @return The 16-bit captured value, or 0 for an unknown instance.
 */
uint16_t EPIC_CCP_GetCapture(CCP_InstanceTypeDef inst)
{
    if (inst != CCP_INSTANCE_1 && inst != CCP_INSTANCE_2) return 0U;
    uint8_t lo, hi1, hi2;
    do
    {
        CCP_READ_CPRH(inst, hi1);
        CCP_READ_CPRL(inst, lo);
        CCP_READ_CPRH(inst, hi2);
    } while (hi1 != hi2);
    return (uint16_t)(((uint16_t)hi2 << 8) | lo);
}

/**
 * @brief  Set the PWM duty in 10-bit units (0..1023). Writes the LSBs into
 *         CCPxCON<5:4> then CCPRxL (bits 9:2), preserving the mode and P1M
 *         bits (DS39632E §16.4.4).
 * @param inst CCP instance.
 * @param duty 10-bit duty cycle value.
 */
void EPIC_CCP_SetPWMDuty(CCP_InstanceTypeDef inst, uint16_t duty)
{
    if (inst != CCP_INSTANCE_1 && inst != CCP_INSTANCE_2) return;
    duty &= 0x03FFU;
    /* Latch the duty LSBs first (CCPxCON<5:4>), then CCPRxL (bits 9:2),
     * preserving the mode + P1M bits (DS39632E §16.4.4). */
    uint8_t con;
    CCP_READ_CON(inst, con);
    con = (uint8_t)(con & ~PIC_CCP1_DC1B_MASK);
    con |= (uint8_t)((duty & 0x03U) << 4);
    CCP_WRITE_CON(inst, con);
    CCP_WRITE_CPRL(inst, duty >> 2);
}

/**
 * @brief  Configure the ECCP1 dead-band delay and auto-restart
 *         (ECCP1DEL). No-op for CCP2. Relevant in half-bridge /
 *         full-bridge PWM modes.
 * @param inst CCP instance (only CCP_INSTANCE_1 is acted on).
 * @param delay Dead-band delay count (PDC<6:0> field).
 * @param auto_restart Whether to auto-restart the PWM after shutdown
 *                     (PRSEN bit).
 */
void EPIC_CCP_ConfigDeadBand(CCP_InstanceTypeDef inst,
                            uint8_t delay, bool auto_restart)
{
    if (inst != CCP_INSTANCE_1) return;     /* ECCP1 only */
    uint8_t del = (uint8_t)(delay & PIC_ECCP1DEL_PDC_MASK);
    if (auto_restart) del |= PIC_ECCP1DEL_PRSEN;
    EPIC_REG8(PIC_REG_ECCP1DEL) = del;
}

/**
 * @brief  Configure the ECCP1 auto-shutdown source and pin states
 *         (ECCP1AS). No-op for CCP2. Pass `CCP_AUTOSHUTDOWN_DISABLED` to
 *         turn it off.
 * @param inst CCP instance (only CCP_INSTANCE_1 is acted on).
 * @param source Auto-shutdown trigger source (SS<2:0> field).
 * @param pins_ac Pin states for the P1A/P1C pins on shutdown (PSSAC).
 * @param pins_bd Pin states for the P1B/P1D pins on shutdown (PSSBD).
 */
void EPIC_CCP_ConfigAutoShutdown(CCP_InstanceTypeDef inst,
                                CCP_AutoShutdownSourceTypeDef source,
                                CCP_PinStateTypeDef pins_ac,
                                CCP_PinStateTypeDef pins_bd)
{
    if (inst != CCP_INSTANCE_1) return;     /* ECCP1 only */
    /* Preserve ECCPASE (status); reprogram source + pin states. */
    uint8_t asv = (uint8_t)(epic_sfr_read8(PIC_REG_ECCP1AS) & PIC_ECCP1AS_ECCPASE);
    asv |= (uint8_t)(((source & 0x7U) << 4) |
                     (pss_encode(pins_ac) << 2) |
                     pss_encode(pins_bd));
    epic_sfr_write8(PIC_REG_ECCP1AS, asv);
}

/**
 * @brief  Return 1 if an ECCP1 auto-shutdown event is active
 *         (ECCP1AS<ECCPASE>). Always 0 for CCP2.
 * @param inst CCP instance.
 * @return 1 if shutdown is active, else 0.
 */
uint8_t EPIC_CCP_IsShutdown(CCP_InstanceTypeDef inst)
{
    if (inst != CCP_INSTANCE_1) return 0U;
    return (EPIC_REG8(PIC_REG_ECCP1AS) & PIC_ECCP1AS_ECCPASE) ? 1U : 0U;
}

/**
 * @brief  Clear the ECCP1 auto-shutdown status (ECCPASE) to restart the
 *         PWM. Only effective when PRSEN = 0 (manual restart); with
 *         PRSEN = 1 the hardware auto-clears when the shutdown source
 *         deasserts. No-op for CCP2.
 * @param inst CCP instance.
 */
void EPIC_CCP_Restart(CCP_InstanceTypeDef inst)
{
    if (inst != CCP_INSTANCE_1) return;
    /* Clear ECCPASE, preserving the source/pin-state configuration. */
    uint8_t asv = (uint8_t)(epic_sfr_read8(PIC_REG_ECCP1AS) & ~PIC_ECCP1AS_ECCPASE);
    epic_sfr_write8(PIC_REG_ECCP1AS, asv);
}

/**
 * @brief  Weak CCP1 (ECCP1) interrupt handler: clears CCP1IF and invokes
 *         the event callback registered via Init.
 */
void CCP1_IRQHandler(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR1), PIC_PIR1_CCP1IF);
    if (g_ccp_storage[CCP_INSTANCE_1].EventCallback)
    {
        g_ccp_storage[CCP_INSTANCE_1].EventCallback();
    }
}

/**
 * @brief  Weak CCP2 interrupt handler: clears CCP2IF and invokes the
 *         event callback registered via Init.
 */
void CCP2_IRQHandler(void)
{
    EPIC_BIT_CLR(EPIC_REG8(PIC_REG_PIR2), PIC_PIR2_CCP2IF);
    if (g_ccp_storage[CCP_INSTANCE_2].EventCallback)
    {
        g_ccp_storage[CCP_INSTANCE_2].EventCallback();
    }
}
