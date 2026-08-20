// Milestone-9 multi-bank FSR acceptance: arrays pushed into banks 1-3 are
// written and read through the FSR+IRP path at a runtime index, with a
// banked direct copy, an sret call into a frame alloca, and a chained
// dynamic index in the same program.
//
// Layout (alloc, region_for): `in` (i16) at 0x20-0x21; filler[78] fills the
// rest of bank 0 (0x22-0x6F), so arrB1 lands at 0xA0 (bank 1), fill1[64]
// fills bank 1 (0xB0-0xEF), arrB2 at 0x120 (bank 2), fill2[64] fills bank 2
// (0x130-0x16F), arrB3 at 0x1A0 (bank 3) and out at 0x1B0 (bank 3). Each
// array's base + 16-byte span sits entirely inside one FSR+IRP window
// ([0xA0,0xF0) / [0x120,0x170) / [0x1A0,0x1F0)), so the window asserts
// cannot fire. The fill arrays exist only to push the three arrB arrays
// across the bank boundaries; they are touched (volatile stores) so clang
// -O1 keeps declaration order (clang reorders *unused* globals to the end,
// which would leave the arrays in bank 0).
//
// `in` is a 16-bit volatile so clang keeps the index mask `& 3` as an i16
// `and` (isel lowers i16 and; it has no i8 and) , same discipline as
// array.c/ptr_probe.c. The brief's draft used a constant `i = 3`, which
// clang -O1 folds into constant GEPs (killing the FSR coverage); the
// runtime index `i = in & 3` keeps all three arrB[i] accesses dynamic.
//
// The brief's draft also copied arrB2[5] = arrB1[1] *before* writing
// arrB1[1] = 0x07, which makes the direct copy copy a stale 0 (out would
// end 0xB1, not 0xB8 , the plan's own comment claims 0x66 + 0x07 = 0x6D).
// Reordered so the banked direct copy carries the live 0x07, preserving the
// plan's coverage (banked direct copy BANKSEL read bank1 -> write bank2)
// and the intended final value.
//
// Expected: out == 0xB8 for in == 3 (hand trace in banked_ptr_e2e.rs).
struct P { unsigned char a; unsigned char b; };

volatile unsigned short in;          // 0x20-0x21 (bank 0): index input
volatile unsigned char filler[78];   // 0x22-0x6F: fills bank 0 (region_for)
volatile unsigned char arrB1[16];    // 0xA0-0xAF (bank 1)
volatile unsigned char fill1[64];    // 0xB0-0xEF: fills bank 1
volatile unsigned char arrB2[16];    // 0x120-0x12F (bank 2)
volatile unsigned char fill2[64];    // 0x130-0x16F: fills bank 2
volatile unsigned char arrB3[16];    // 0x1A0-0x1AF (bank 3)
volatile unsigned char out;          // 0x1B0 (bank 3)

__attribute__((noinline)) struct P mk(void) {  // sret into a frame alloca
    struct P r; r.a = 5; r.b = 6; return r;
}

void main(void) {
    // clang -O1 emits globals in first-use order (unused ones last), so the
    // touches below are interleaved to pin each global's slot: in, filler,
    // arrB1, fill1, arrB2, fill2, arrB3, out (see the layout comment above).
    unsigned char i = (unsigned char)(in & 3);
    filler[0] = 0x01;                            // touch filler (bank 0)
    arrB1[i] = 0x11;                             // FSR+IRP write (bank 1)
    fill1[0] = 0x02;                             // touch fill1 (bank 1)
    arrB2[i] = 0x22;                             // FSR+IRP write (bank 2)
    fill2[0] = 0x04;                             // touch fill2 (bank 2)
    arrB3[i] = 0x33;                             // FSR+IRP write (bank 3)
    out = arrB1[i] + arrB2[i] + arrB3[i];                // 0x66 , FSR+IRP reads
    arrB1[1] = 0x07;
    arrB2[5] = arrB1[1];                                 // banked direct copy (BANKSEL)
    out = (unsigned char)(out + arrB2[5]);               // 0x66 + 0x07 = 0x6D
    struct P g;                                          // alloca; sret target
    g = mk();                                            // sret call into the frame
    out = (unsigned char)(out + g.a + g.b);              // 0x6D + 5 + 6 = 0x78
    arrB3[arrB2[2]] = 0x40;                              // chained dynamic index
    out = (unsigned char)(out + arrB3[0]);               // 0x78 + 0x40 = 0xB8
}
