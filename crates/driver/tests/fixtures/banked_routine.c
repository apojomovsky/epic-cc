// Issue #6 acceptance: a runtime routine whose whole frame lands in a
// non-zero GPR bank.
//
// Layout: `out` u16 at 0x20-0x21, `g[30]` (60 bytes) at 0x22-0x5D ->
// end_of_globals 0x5E. main is lean (one i16 def); the noinline helper
// `mul30` carries ~120 bytes of i16 defs (volatile loads + adds), pushing
// its frame well past the bank-0/1 gap so that __mul_u16's frame, derived
// at the helper's physical end, sits entirely inside a higher bank.
// Before issue #6 the isel assert rejected every slot > 0x7F with a loud
// panic; now the routine frame lives in one non-zero bank and the banking
// pass selects that bank around the recipe call without ever touching the
// skip-sensitive loops.
//
// Hand computation: g[i] seeded 1..30, sum = 30*31/2 = 465,
// out = (465 * 7) & 0xFFFF = 3255 = 0x0CB7.
volatile unsigned int out;
volatile unsigned int g[30];

__attribute__((noinline)) unsigned int mul30(void) {
    unsigned int s = g[0];
    s += g[1];
    s += g[2];
    s += g[3];
    s += g[4];
    s += g[5];
    s += g[6];
    s += g[7];
    s += g[8];
    s += g[9];
    s += g[10];
    s += g[11];
    s += g[12];
    s += g[13];
    s += g[14];
    s += g[15];
    s += g[16];
    s += g[17];
    s += g[18];
    s += g[19];
    s += g[20];
    s += g[21];
    s += g[22];
    s += g[23];
    s += g[24];
    s += g[25];
    s += g[26];
    s += g[27];
    s += g[28];
    s += g[29];
    return s * 7u;
}

void main(void) {
    g[0] = 1u;
    g[1] = 2u;
    g[2] = 3u;
    g[3] = 4u;
    g[4] = 5u;
    g[5] = 6u;
    g[6] = 7u;
    g[7] = 8u;
    g[8] = 9u;
    g[9] = 10u;
    g[10] = 11u;
    g[11] = 12u;
    g[12] = 13u;
    g[13] = 14u;
    g[14] = 15u;
    g[15] = 16u;
    g[16] = 17u;
    g[17] = 18u;
    g[18] = 19u;
    g[19] = 20u;
    g[20] = 21u;
    g[21] = 22u;
    g[22] = 23u;
    g[23] = 24u;
    g[24] = 25u;
    g[25] = 26u;
    g[26] = 27u;
    g[27] = 28u;
    g[28] = 29u;
    g[29] = 30u;
    out = mul30();
}
