// P3 (PIC14E) acceptance: a bank-straddling array addressed through the
// linear region (docs/33 D-2), alongside a single-bank array addressed
// through the physical (banked) FSR path, in one program. The emitted
// `.asm` must use a linear FSR base (0x2000-0x29AF) for the spanning array
// and a physical base for the single-bank one.
//
// Layout (alloc, region_for): `in` (i16) at 0x20-0x21; `big[90]` straddles
// bank 0 -> bank 1 (0x22-0x6F then 0xA0-0xAF, the common-RAM hole 0x70-0x7F
// skipped), so its linear base is 0x2002 and one FSR walks all 90 bytes;
// `small[8]` fits bank 1 (0xB0-0xB7), addressed physically. `out` at 0xB8.
//
// `in` is a 16-bit volatile so clang keeps the index mask `& 7` as an i16
// `and` (isel lowers i16 and; it has no i8 and), the same discipline as
// array.c/ptr_probe.c. The runtime index keeps the accesses dynamic so the
// FSR path is exercised, not folded to constant GEPs.
//
// Expected: in = 3 -> big[3] = 0x11, big[89] = 0x22, small[3] = 0x33,
// out = 0x11 + 0x22 + 0x33 = 0x66.
volatile unsigned short in;
volatile unsigned char big[90];
volatile unsigned char small[8];
volatile unsigned char out;
void main(void) {
    unsigned char i = (unsigned char)(in & 7);
    big[i] = 0x11;          // linear FSR write (spanning)
    big[89] = 0x22;         // linear FSR write (spanning, far end)
    small[i] = 0x33;        // banked FSR write (single bank)
    out = (unsigned char)(big[i] + big[89] + small[i]);  // 0x11 + 0x22 + 0x33
}
