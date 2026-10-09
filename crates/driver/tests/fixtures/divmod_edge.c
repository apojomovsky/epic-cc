/* Divmod edge coverage (epic-cc#894). Volatile operands force the runtime
 * helpers (the bench shape); defined cases self-check against literals,
 * poison cases (divisor zero, INT_MIN/-1) record raw results for the
 * cross-profile equality assert in divmod_edge_e2e.rs. Plain C99. */
#include <stdint.h>

volatile unsigned char fails;
volatile uint8_t a8, b8, q8, m8, p8q, p8m;
volatile uint16_t a16, b16, q16, m16, p16q, p16m;
volatile uint32_t a32, b32, q32, m32, p32q, p32m;
volatile int8_t sa8, sb8, sq8, sm8;
volatile int16_t sa16, sb16, sq16, sm16, ps16q, ps16m;
volatile int32_t sa32, sb32, sq32, sm32, ps32q, ps32m;

void main(void) {
    /* u8 defined. */
    a8 = 200; b8 = 13; q8 = (uint8_t)(a8 / b8); m8 = (uint8_t)(a8 % b8);
    if (q8 != 15) fails++;
    if (m8 != 5) fails++;
    a8 = 0; b8 = 13; q8 = (uint8_t)(a8 / b8); m8 = (uint8_t)(a8 % b8);
    if (q8 != 0) fails++;
    if (m8 != 0) fails++;
    a8 = 5; b8 = 200; q8 = (uint8_t)(a8 / b8); m8 = (uint8_t)(a8 % b8);
    if (q8 != 0) fails++;
    if (m8 != 5) fails++;
    a8 = 255; b8 = 1; q8 = (uint8_t)(a8 / b8); m8 = (uint8_t)(a8 % b8);
    if (q8 != 255) fails++;
    if (m8 != 0) fails++;
    a8 = 255; b8 = 255; q8 = (uint8_t)(a8 / b8); m8 = (uint8_t)(a8 % b8);
    if (q8 != 1) fails++;
    if (m8 != 0) fails++;
    /* u8 poison: divisor zero records only. */
    a8 = 200; b8 = 0; p8q = (uint8_t)(a8 / b8); p8m = (uint8_t)(a8 % b8);
    /* u16 defined. */
    a16 = 50000; b16 = 137; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 364) fails++;
    if (m16 != 132) fails++;
    a16 = 0; b16 = 137; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 0) fails++;
    if (m16 != 0) fails++;
    a16 = 5; b16 = 50000; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 0) fails++;
    if (m16 != 5) fails++;
    a16 = 65535; b16 = 1; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 65535) fails++;
    if (m16 != 0) fails++;
    a16 = 65535; b16 = 65535; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 1) fails++;
    if (m16 != 0) fails++;
    a16 = 1; b16 = 1; q16 = (uint16_t)(a16 / b16); m16 = (uint16_t)(a16 % b16);
    if (q16 != 1) fails++;
    if (m16 != 0) fails++;
    /* u16 poison. */
    a16 = 50000; b16 = 0; p16q = (uint16_t)(a16 / b16); p16m = (uint16_t)(a16 % b16);
    /* u32 defined. */
    a32 = 3000000000UL; b32 = 1234567UL; q32 = a32 / b32; m32 = a32 % b32;
    if (q32 != 2430UL) fails++;
    if (m32 != 2190UL) fails++;
    a32 = 0; b32 = 1234567UL; q32 = a32 / b32; m32 = a32 % b32;
    if (q32 != 0) fails++;
    if (m32 != 0) fails++;
    a32 = 5; b32 = 4000000000UL; q32 = a32 / b32; m32 = a32 % b32;
    if (q32 != 0) fails++;
    if (m32 != 5) fails++;
    a32 = 4294967295UL; b32 = 1; q32 = a32 / b32; m32 = a32 % b32;
    if (q32 != 4294967295UL) fails++;
    if (m32 != 0) fails++;
    a32 = 4294967295UL; b32 = 4294967295UL; q32 = a32 / b32; m32 = a32 % b32;
    if (q32 != 1) fails++;
    if (m32 != 0) fails++;
    /* u32 poison. */
    a32 = 3000000000UL; b32 = 0; p32q = a32 / b32; p32m = a32 % b32;
    /* s8 defined. */
    sa8 = -100; sb8 = 7; sq8 = (int8_t)(sa8 / sb8); sm8 = (int8_t)(sa8 % sb8);
    if (sq8 != -14) fails++;
    if (sm8 != -2) fails++;
    sa8 = 100; sb8 = -7; sq8 = (int8_t)(sa8 / sb8); sm8 = (int8_t)(sa8 % sb8);
    if (sq8 != -14) fails++;
    if (sm8 != 2) fails++;
    sa8 = 0; sb8 = 5; sq8 = (int8_t)(sa8 / sb8); sm8 = (int8_t)(sa8 % sb8);
    if (sq8 != 0) fails++;
    if (sm8 != 0) fails++;
    sa8 = 127; sb8 = 127; sq8 = (int8_t)(sa8 / sb8); sm8 = (int8_t)(sa8 % sb8);
    if (sq8 != 1) fails++;
    if (sm8 != 0) fails++;
    /* s16 defined. */
    sa16 = -19; sb16 = -3; sq16 = (int16_t)(sa16 / sb16); sm16 = (int16_t)(sa16 % sb16);
    if (sq16 != 6) fails++;
    if (sm16 != -1) fails++;
    sa16 = -19; sb16 = 3; sq16 = (int16_t)(sa16 / sb16); sm16 = (int16_t)(sa16 % sb16);
    if (sq16 != -6) fails++;
    if (sm16 != -1) fails++;
    sa16 = 19; sb16 = -3; sq16 = (int16_t)(sa16 / sb16); sm16 = (int16_t)(sa16 % sb16);
    if (sq16 != -6) fails++;
    if (sm16 != 1) fails++;
    sa16 = 0; sb16 = -5; sq16 = (int16_t)(sa16 / sb16); sm16 = (int16_t)(sa16 % sb16);
    if (sq16 != 0) fails++;
    if (sm16 != 0) fails++;
    /* s16 poison: divisor zero and INT_MIN/-1 record only. */
    sa16 = -19; sb16 = 0; ps16q = (int16_t)(sa16 / sb16); ps16m = (int16_t)(sa16 % sb16);
    sa16 = -32768; sb16 = -1; ps16q = (int16_t)(sa16 / sb16); ps16m = (int16_t)(sa16 % sb16);
    /* s32 defined. */
    sa32 = -2000000000L; sb32 = 1234567L; sq32 = sa32 / sb32; sm32 = sa32 % sb32;
    if (sq32 != -1620L) fails++;
    if (sm32 != -1460L) fails++;
    sa32 = 2000000000L; sb32 = -1234567L; sq32 = sa32 / sb32; sm32 = sa32 % sb32;
    if (sq32 != -1620L) fails++;
    if (sm32 != 1460L) fails++;
    /* s32 poison. */
    sa32 = (-2147483647L - 1); sb32 = -1; ps32q = sa32 / sb32; ps32m = sa32 % sb32;
}
