/*
 * PIC18 inline-asm add/sub/negate primitives using PIC18's
 * single-instruction carry ops (addwfc/subwfb), unlike PIC16's
 * skip-and-increment idiom. Operands are file-scope static volatile
 * scratch, one banksel per routine (see ARCHITECTURE.md "Inline-asm
 * binding").
 */

#include <xc.h>
#include "epic_math.h"

/* 16-bit add. Low bytes add (sets C); addwfc folds the carry into the
 * high byte, which movf preserves. Worked example 0xFFFF+0x0002 ->
 * 0x0001, carry 1 pins the invariant that the fold stays C-correct when
 * the low add overflows. */
static volatile uint16_t m_add_a, m_add_b, m_add_r;
static volatile uint8_t  m_add_co;

/**
 * @brief  16-bit unsigned add with carry out (PIC18 inline asm, using
 *         addwfc).
 * @param  a           augend, 0..65535
 * @param  b           addend, 0..65535
 * @param  carry_out  set true on overflow (sum > 65535); may be NULL.
 * @return (a + b) truncated to 16 bits.
 */
uint16_t epic_math_add_u16(uint16_t a, uint16_t b, bool *carry_out)
{
    m_add_a = a; m_add_b = b; m_add_co = 0;
    asm("banksel _m_add_a");
    asm("movf   _m_add_b+0,w");
    asm("addwf  _m_add_a+0,w");        /* w = aLO + bLO, C = carry-out        */
    asm("movwf  _m_add_r+0");          /* rLO                                 */
    asm("movf   _m_add_b+1,w");        /* w = bHI (movf preserves C)          */
    asm("addwfc _m_add_a+1,w");        /* w = aHI + bHI + C, C = final carry  */
    asm("movwf  _m_add_r+1");          /* rHI                                 */
    asm("clrf   _m_add_co");
    asm("btfsc  STATUS,0");            /* if C=1 (carry out)                  */
    asm("setf   _m_add_co");
    if (carry_out) *carry_out = (bool)m_add_co;
    return m_add_r;
}

/* a - b, borrow_out = (a < b). Low bytes subtract (C=0 on borrow);
 * subwfb folds the borrow into the high subtract. Borrow is the final C
 * inverted. */
static volatile uint16_t m_sub_a, m_sub_b, m_sub_r;
static volatile uint8_t  m_sub_bo;

/**
 * @brief  16-bit unsigned subtract with borrow out (PIC18 inline asm,
 *         using subwfb).
 * @param  a           minuend, 0..65535
 * @param  b           subtrahend, 0..65535
 * @param  borrow_out  set true on underflow (a < b); may be NULL.
 * @return (a - b) truncated to 16 bits.
 */
uint16_t epic_math_sub_u16(uint16_t a, uint16_t b, bool *borrow_out)
{
    m_sub_a = a; m_sub_b = b; m_sub_bo = 0;
    asm("banksel _m_sub_a");
    asm("movf    _m_sub_b+0,w");
    asm("subwf   _m_sub_a+0,w");       /* w = aLO - bLO, C=1 if no borrow    */
    asm("movwf   _m_sub_r+0");         /* rLO                                */
    asm("movf    _m_sub_b+1,w");       /* w = bHI (C preserved)              */
    asm("subwfb  _m_sub_a+1,w");       /* w = aHI - bHI - borrow, C=1 if ok  */
    asm("movwf   _m_sub_r+1");         /* rHI                                */
    asm("clrf    _m_sub_bo");
    asm("btfss   STATUS,0");           /* if C=0 (borrowed)                  */
    asm("setf    _m_sub_bo");
    if (borrow_out) *borrow_out = (bool)m_sub_bo;
    return m_sub_r;
}

/* -v = ~v + 1: complement both bytes, inc low, and inc high only if the
 * low inc wrapped (Z). INT16_MIN negates to itself. */
static volatile int16_t m_neg_v;
static volatile int16_t m_neg_r;

/**
 * @brief  16-bit two's-complement negate (PIC18 inline asm).
 * @param  v  value to negate, -32768..32767
 * @return -v; INT16_MIN negates to itself (two's-complement wrap).
 */
int16_t epic_math_negate_s16(int16_t v)
{
    m_neg_v = v;
    asm("banksel _m_neg_v");
    asm("comf  _m_neg_v+0,w");
    asm("movwf _m_neg_r+0");
    asm("comf  _m_neg_v+1,w");
    asm("movwf _m_neg_r+1");
    asm("incf  _m_neg_r+0,f");         /* rLO++ ; Z if wrapped to 0           */
    asm("btfsc STATUS,2");             /* if Z (rLO wrapped), inc rHI         */
    asm("incf  _m_neg_r+1,f");
    return m_neg_r;
}

/* Same ~v + 1 with the carry cascade across 4 bytes: each btfsc reads
 * the Z from the preceding incf (a skipped incf leaves Z=0, so the
 * cascade stops after the first non-wrap). */
static volatile int32_t m_neg32_v;
static volatile int32_t m_neg32_r;

/**
 * @brief  32-bit two's-complement negate (PIC18 inline asm).
 * @param  v  value to negate, -2147483648..2147483647
 * @return -v; INT32_MIN negates to itself (two's-complement wrap).
 */
int32_t epic_math_negate_s32(int32_t v)
{
    m_neg32_v = v;
    asm("banksel _m_neg32_v");
    asm("comf  _m_neg32_v+0,w");  asm("movwf _m_neg32_r+0");
    asm("comf  _m_neg32_v+1,w");  asm("movwf _m_neg32_r+1");
    asm("comf  _m_neg32_v+2,w");  asm("movwf _m_neg32_r+2");
    asm("comf  _m_neg32_v+3,w");  asm("movwf _m_neg32_r+3");
    asm("incf  _m_neg32_r+0,f");         /* byte0++ ; Z if wrapped          */
    asm("btfsc STATUS,2");               /* cascade carry while Z set       */
    asm("incf  _m_neg32_r+1,f");
    asm("btfsc STATUS,2");
    asm("incf  _m_neg32_r+2,f");
    asm("btfsc STATUS,2");
    asm("incf  _m_neg32_r+3,f");
    return m_neg32_r;
}
