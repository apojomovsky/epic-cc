/*
 * PIC18 inline-asm divide/modulo primitives via the same restoring
 * shift-subtract algorithm as AN526/AN544 (neither core has hardware
 * divide): num shifts left into rem, rem reduces by den when it fits,
 * ORing a quotient bit into num's LSB. Signed divide is plain C
 * negate/negate-result over the asm unsigned path, matching the C99
 * truncated-division identity in the header docs.
 */

#include <xc.h>
#include <stddef.h>     /* NULL */
#include "epic_math.h"

/* 16/16 restoring division, 16 iterations. For a 16-bit dividend the
 * partial remainder never reaches 0x8000 (the partial dividend is
 * <= 0xFFFF, so remainder = partial mod den < 0x8000), so the rem shift
 * never carries out and a plain 16-bit compare/subtract is correct.
 *
 * One iteration, mirroring ref_divmod_u16_algo:
 *   c = num>>15; num <<= 1;            -- bcf C; rlcf num+0; rlcf num+1
 *   rem = (rem<<1)|c;                  -- rlcf rem+0; rlcf rem+1 (C was c)
 *   if (rem >= den) { rem -= den; num |= 1; }
 *     done in place: subtract den from rem (C=1 if rem>=den); if C=1 set
 *     num LSB and keep, else restore rem += den (quotient bit stays 0).
 *
 * Worked example 0x0007/0x0002 -> 3 r 1: only the dividend's three 1-bits
 * can make rem >= 2, and each produces a quotient bit; the final three
 * iterations give num 0xE000 -> 0xC000 -> 0x8001 -> 0x0003, rem 1. */
static volatile uint16_t m_du16_num, m_du16_rem, m_du16_den;
static volatile uint8_t  m_du16_cnt;

/**
 * @brief  Unsigned 16/16 divide with remainder (PIC18 inline asm,
 *         restoring shift-subtract).
 * @param  num  numerator,   0..65535
 * @param  den  denominator, 1..65535 (0 -> *ok=false, fields zeroed)
 * @param  ok   out: true if den != 0; may be NULL
 * @return { quotient = num/den, remainder = num%den }.
 *
 * 16/16 restoring division, 16 iterations. For a 16-bit dividend the
 * partial remainder never reaches 0x8000 (the partial dividend is
 * <= 0xFFFF, so remainder = partial mod den < 0x8000), so the rem shift
 * never carries out and a plain 16-bit compare/subtract is correct.
 *
 * One iteration, mirroring ref_divmod_u16_algo:
 *   c = num>>15; num <<= 1;            -- bcf C; rlcf num+0; rlcf num+1
 *   rem = (rem<<1)|c;                  -- rlcf rem+0; rlcf rem+1 (C was c)
 *   if (rem >= den) { rem -= den; num |= 1; }
 *     done in place: subtract den from rem (C=1 if rem>=den); if C=1 set
 *     num LSB and keep, else restore rem += den (quotient bit stays 0).
 *
 * Worked example 0x0007/0x0002 -> 3 r 1: only the dividend's three 1-bits
 * can make rem >= 2, and each produces a quotient bit; the final three
 * iterations give num 0xE000 -> 0xC000 -> 0x8001 -> 0x0003, rem 1. */
epic_math_udiv16_t epic_math_divmod_u16(uint16_t num, uint16_t den, bool *ok)
{
    epic_math_udiv16_t res = { 0u, 0u };
    if (den == 0u)
    {
        if (ok) *ok = false; return res;
    }
    if (ok) *ok = true;

    m_du16_num = num; m_du16_den = den;
    asm("banksel _m_du16_num");
    asm("clrf  _m_du16_rem+0");
    asm("clrf  _m_du16_rem+1");
    asm("movlw 16");
    asm("movwf _m_du16_cnt");
    asm("_du16_loop:");
    /* shift num left 1, MSB -> C, LSB <- 0 */
    asm("bcf   STATUS,0");
    asm("rlcf  _m_du16_num+0,f");
    asm("rlcf  _m_du16_num+1,f");
    /* shift rem left 1, LSB <- C (the dividend bit) */
    asm("rlcf  _m_du16_rem+0,f");
    asm("rlcf  _m_du16_rem+1,f");
    /* subtract den from rem in place; C=1 if rem>=den (no borrow) */
    asm("movf  _m_du16_den+0,w");
    asm("subwf _m_du16_rem+0,f");
    asm("movf  _m_du16_den+1,w");
    asm("btfss STATUS,0");          /* if low borrow (C=0), add 1 to subtrahend hi */
    asm("addlw 1");
    asm("subwf _m_du16_rem+1,f");   /* C=1 if rem>=den overall */
    /* C=1: success -> set quotient bit; C=0: borrow -> restore rem += den */
    asm("btfss STATUS,0");
    asm("bra   _du16_restore");
    asm("bsf   _m_du16_num+0,0");   /* quotient bit = 1 */
    asm("bra   _du16_next");
    asm("_du16_restore:");
    asm("movf  _m_du16_den+0,w");
    asm("addwf _m_du16_rem+0,f");
    asm("movf  _m_du16_den+1,w");
    asm("addwfc _m_du16_rem+1,f");
    asm("_du16_next:");
    asm("decfsz _m_du16_cnt,f");
    asm("bra   _du16_loop");

    res.quotient = m_du16_num;
    res.remainder = m_du16_rem;
    return res;
}

/* 32/16 restoring division, 32 iterations. Unlike 16/16, the partial
 * remainder CAN reach 0x8000 (partial dividend up to 0xFFFFFFFF), so the
 * rem shift can carry out of bit 15: the extended remainder is C:rem
 * (17 bits), and the compare is "extended >= den", i.e. C==1 OR rem>=den.
 * If the shift carried, subtract unconditionally (the 16-bit rem wraps to
 * the correct low-16 of extended-den) and set the bit; otherwise do the
 * plain 16-bit restoring subtract. Mirrors ref_divmod_u32_16_algo
 * (tested). Quotient is truncated to 16 bits by returning the low word of
 * the 32-bit quotient accumulator. */
static volatile uint32_t m_du32_num;
static volatile uint16_t m_du32_rem, m_du32_den;
static volatile uint8_t  m_du32_cnt;

/**
 * @brief  Wide unsigned 32/16 divide with remainder (PIC18 inline asm,
 *         restoring shift-subtract); quotient truncated to 16 bits
 *         (documented contract).
 * @param  num  numerator,   0..0xFFFFFFFF
 * @param  den  denominator, 1..65535 (0 -> *ok=false, fields zeroed)
 * @param  ok   out: true if den != 0; may be NULL
 * @return { quotient = low 16 bits of num/den, remainder = num%den }.
 *
 * 32/16 restoring division, 32 iterations. Unlike 16/16, the partial
 * remainder CAN reach 0x8000 (partial dividend up to 0xFFFFFFFF), so the
 * rem shift can carry out of bit 15: the extended remainder is C:rem
 * (17 bits), and the compare is "extended >= den", i.e. C==1 OR rem>=den.
 * If the shift carried, subtract unconditionally (the 16-bit rem wraps to
 * the correct low-16 of extended-den) and set the bit; otherwise do the
 * plain 16-bit restoring subtract. Mirrors ref_divmod_u32_16_algo
 * (tested). Quotient is truncated to 16 bits by returning the low word of
 * the 32-bit quotient accumulator. */
epic_math_udiv16_t epic_math_divmod_u32_16(uint32_t num, uint16_t den, bool *ok)
{
    epic_math_udiv16_t res = { 0u, 0u };
    if (den == 0u)
    {
        if (ok) *ok = false; return res;
    }
    if (ok) *ok = true;

    m_du32_num = num; m_du32_den = den;
    asm("banksel _m_du32_num");
    asm("clrf  _m_du32_rem+0");
    asm("clrf  _m_du32_rem+1");
    asm("movlw 32");
    asm("movwf _m_du32_cnt");
    asm("_du32_loop:");
    /* shift the 32-bit num left 1, MSB -> C, LSB <- 0 */
    asm("bcf   STATUS,0");
    asm("rlcf  _m_du32_num+0,f");
    asm("rlcf  _m_du32_num+1,f");
    asm("rlcf  _m_du32_num+2,f");
    asm("rlcf  _m_du32_num+3,f");
    /* shift rem left 1, LSB <- C; the bit shifted OUT of rem is the 17th */
    asm("rlcf  _m_du32_rem+0,f");
    asm("rlcf  _m_du32_rem+1,f");  /* C now = old MSB of rem (the 17th bit)  */
    /* if C=1 (extended >= 0x10000 > den): subtract unconditionally, bit=1 */
    asm("btfss STATUS,0");
    asm("bra   _du32_cmp");
    asm("movf  _m_du32_den+0,w");
    asm("subwf _m_du32_rem+0,f");
    asm("movf  _m_du32_den+1,w");
    asm("btfss STATUS,0");
    asm("addlw 1");
    asm("subwf _m_du32_rem+1,f");  /* rem wraps to low-16 of (extended-den)  */
    asm("bsf   _m_du32_num+0,0"); /* quotient bit = 1 */
    asm("bra   _du32_next");
    /* C=0: plain 16-bit restoring subtract */
    asm("_du32_cmp:");
    asm("movf  _m_du32_den+0,w");
    asm("subwf _m_du32_rem+0,f");
    asm("movf  _m_du32_den+1,w");
    asm("btfss STATUS,0");
    asm("addlw 1");
    asm("subwf _m_du32_rem+1,f");
    asm("btfss STATUS,0");        /* if C=1 (rem>=den), skip restore */
    asm("bra   _du32_restore");
    asm("bsf   _m_du32_num+0,0");
    asm("bra   _du32_next");
    asm("_du32_restore:");
    asm("movf  _m_du32_den+0,w");
    asm("addwf _m_du32_rem+0,f");
    asm("movf  _m_du32_den+1,w");
    asm("addwfc _m_du32_rem+1,f");
    asm("_du32_next:");
    asm("decfsz _m_du32_cnt,f");
    asm("bra   _du32_loop");

    res.quotient = (uint16_t)m_du32_num;        /* low 16 bits = truncated quotient */
    res.remainder = m_du32_rem;
    return res;
}

/**
 * @brief  Signed 16/16 divide with remainder over the asm unsigned path
 *         (PIC18).
 * @param  num  numerator,   -32768..32767
 * @param  den  denominator, nonzero (0 -> *ok=false, fields zeroed)
 * @param  ok   out: true if den != 0; may be NULL
 * @return { quotient = num/den, remainder = num%den }.
 *
 * abs the operands (unsigned, so INT16_MIN abs = 0x8000 with no
 * 16-bit-int overflow), call divmod_u16, then apply the sign. Quotient
 * sign = sign(num) ^ sign(den); remainder sign follows the dividend (C99
 * truncated division: (a/b)*b + a%b == a). INT16_MIN / -1 -> quotient
 * 0x8000 (the wrap of 32768), remainder 0: the one signed divide that
 * can overflow int16, documented in the header. */
epic_math_sdiv16_t epic_math_divmod_s16(int16_t num, int16_t den, bool *ok)
{
    epic_math_sdiv16_t res = { 0, 0 };
    if (den == 0)
    {
        if (ok) *ok = false; return res;
    }
    if (ok) *ok = true;

    int neg_q = ((num < 0) != 0) ^ ((den < 0) != 0);
    uint16_t unum = (num < 0) ? (uint16_t)(0u - (uint16_t)num) : (uint16_t)num;
    uint16_t uden = (den < 0) ? (uint16_t)(0u - (uint16_t)den) : (uint16_t)den;

    epic_math_udiv16_t u = epic_math_divmod_u16(unum, uden, NULL);
    res.quotient  = (int16_t)(neg_q ? (uint16_t)(0u - u.quotient)  : u.quotient);
    res.remainder = (int16_t)((num < 0) ? (uint16_t)(0u - u.remainder) : u.remainder);
    return res;
}
