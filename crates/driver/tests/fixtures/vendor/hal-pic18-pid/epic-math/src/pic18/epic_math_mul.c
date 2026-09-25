/*
 * PIC18 inline-asm multiply primitives via the hardware single-cycle
 * MULWF (result in PRODH:PRODL): direct for 8x8, four summed 8x8 partial
 * products for 16x16 (smaller and faster than a shift-add loop or XC8's
 * generic ___lmul runtime call). Signed multiply is the app notes'
 * negate/negate-result trick in plain C over the unsigned asm path.
 */

#include <xc.h>
#include "epic_math.h"

/* 8x8 -> 16 via one MULWF. Worked example 0x0C*0x14 = 0x00F0 pins that
 * PRODL is the low product byte and PRODH the high one. */
static volatile uint8_t  m_mul8_a, m_mul8_b;
static volatile uint16_t m_mul8_r;

/**
 * @brief  8x8 -> 16 unsigned multiply via one MULWF (PIC18 inline asm).
 * @param  a  multiplicand, 0..255
 * @param  b  multiplier,   0..255
 * @return a*b as a 16-bit value, 0..65535 (PRODH:PRODL).
 *
 * Worked example 0x0C*0x14 = 0x00F0 pins that PRODL is the low product
 * byte and PRODH the high one. */
uint16_t epic_math_mul_u8(uint8_t a, uint8_t b)
{
    m_mul8_a = a; m_mul8_b = b;
    asm("banksel _m_mul8_a");
    asm("movf  _m_mul8_a,w");          /* W = a                              */
    asm("mulwf _m_mul8_b");             /* PRODH:PRODL = a * b                */
    asm("movf  PRODL,w");               /* W = low product                    */
    asm("movwf _m_mul8_r+0");
    asm("movf  PRODH,w");               /* W = high product                   */
    asm("movwf _m_mul8_r+1");
    return m_mul8_r;
}

/* 16x16 -> 32 from four 8x8 partial products via MULWF, summed at the
 * right byte offsets with carry propagation. Result bytes r0..r3 (r0 low).
 *
 *   r = aL*bL              (added at r0..r1, carry to r2)
 *     + aL*bH << 8         (added at r1..r2, carry to r3)
 *     + aH*bL << 8         (added at r1..r2, carry to r3)
 *     + aH*bH << 16        (added at r2..r3)
 *
 * Each 16-bit add is: addwf low (C), addwfc high (C), then "movlw 0;
 * addwfc next" to ripple the carry one more byte. The 32-bit product of
 * two 16-bit operands never exceeds 0xFFFE0001, so no carry escapes
 * byte 3. */
static volatile uint16_t m_mul16_a, m_mul16_b;
static volatile uint32_t m_mul16_r;

/**
 * @brief  16x16 -> 32 unsigned multiply from four 8x8 MULWF partial
 *         products summed at the right byte offsets (PIC18 inline asm).
 * @param  a  multiplicand, 0..65535
 * @param  b  multiplier,   0..65535
 * @return a*b as a 32-bit value.
 *
 * Result bytes r0..r3 (r0 low).
 *
 *   r = aL*bL              (added at r0..r1, carry to r2)
 *     + aL*bH << 8         (added at r1..r2, carry to r3)
 *     + aH*bL << 8         (added at r1..r2, carry to r3)
 *     + aH*bH << 16        (added at r2..r3)
 *
 * Each 16-bit add is: addwf low (C), addwfc high (C), then "movlw 0;
 * addwfc next" to ripple the carry one more byte. The 32-bit product of
 * two 16-bit operands never exceeds 0xFFFE0001, so no carry escapes
 * byte 3. */
uint32_t epic_math_mul_u16(uint16_t a, uint16_t b)
{
    m_mul16_a = a; m_mul16_b = b;
    asm("banksel _m_mul16_a");
    asm("clrf  _m_mul16_r+0");
    asm("clrf  _m_mul16_r+1");
    asm("clrf  _m_mul16_r+2");
    asm("clrf  _m_mul16_r+3");

    /* p_LL = aL * bL  -> r0..r1 (carry to r2) */
    asm("movf  _m_mul16_a+0,w");
    asm("mulwf _m_mul16_b+0");
    asm("movf  PRODL,w");  asm("addwf _m_mul16_r+0,f");
    asm("movf  PRODH,w");  asm("addwfc _m_mul16_r+1,f");
    asm("movlw 0");        asm("addwfc _m_mul16_r+2,f");

    /* p_LH = aL * bH  -> r1..r2 (carry to r3) */
    asm("movf  _m_mul16_a+0,w");
    asm("mulwf _m_mul16_b+1");
    asm("movf  PRODL,w");  asm("addwf _m_mul16_r+1,f");
    asm("movf  PRODH,w");  asm("addwfc _m_mul16_r+2,f");
    asm("movlw 0");        asm("addwfc _m_mul16_r+3,f");

    /* p_HL = aH * bL  -> r1..r2 (carry to r3) */
    asm("movf  _m_mul16_a+1,w");
    asm("mulwf _m_mul16_b+0");
    asm("movf  PRODL,w");  asm("addwf _m_mul16_r+1,f");
    asm("movf  PRODH,w");  asm("addwfc _m_mul16_r+2,f");
    asm("movlw 0");        asm("addwfc _m_mul16_r+3,f");

    /* p_HH = aH * bH  -> r2..r3 */
    asm("movf  _m_mul16_a+1,w");
    asm("mulwf _m_mul16_b+1");
    asm("movf  PRODL,w");  asm("addwf _m_mul16_r+2,f");
    asm("movf  PRODH,w");  asm("addwfc _m_mul16_r+3,f");

    return m_mul16_r;
}

/**
 * @brief  16x16 -> 32 signed multiply on top of the unsigned hardware
 *         path (PIC18).
 * @param  a  multiplicand, -32768..32767
 * @param  b  multiplier,   -32768..32767
 * @return (int32_t)a*b.
 *
 * abs the operands (in unsigned, so INT16_MIN abs = 0x8000 with no
 * 16-bit-int overflow), call the asm mul_u16, and negate the 32-bit
 * result if the signs differed. */
int32_t epic_math_mul_s16(int16_t a, int16_t b)
{
    int neg = ((a < 0) != 0) ^ ((b < 0) != 0);
    uint16_t ua = (a < 0) ? (uint16_t)(0u - (uint16_t)a) : (uint16_t)a;
    uint16_t ub = (b < 0) ? (uint16_t)(0u - (uint16_t)b) : (uint16_t)b;
    uint32_t ur = epic_math_mul_u16(ua, ub);
    if (neg)
    {
        ur = (uint32_t)epic_math_negate_s32((int32_t)ur);
    }
    return (int32_t)ur;
}
