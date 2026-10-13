// banked_ptr.c plus `touch`, called right before the constant store through
// a chained dynamic index (epic-cc#1237). `touch` leaves a nonzero byte in
// its pointer param's high byte, the slot the store's zext index lands on:
// if the zext skips its high-byte clear, the FSR add reads that stale byte
// and the 0x40 lands 0x100 bytes off. `touch` only writes arrB3[1], which
// nothing reads, so out is still 0xB8 (layout and trace: see banked_ptr.c).
struct P { unsigned char a; unsigned char b; };

volatile unsigned short in = 3;      // 0x20-0x21 (bank 0): index input
volatile unsigned char filler[78];   // 0x22-0x6F: fills bank 0 (region_for)
volatile unsigned char arrB1[16];    // 0xA0-0xAF (bank 1)
volatile unsigned char fill1[64];    // 0xB0-0xEF: fills bank 1
volatile unsigned char arrB2[16];    // 0x120-0x12F (bank 2)
volatile unsigned char fill2[64];    // 0x130-0x16F: fills bank 2
volatile unsigned char arrB3[16];    // 0x1A0-0x1AF (bank 3)
volatile unsigned char out;          // 0x1B0 (bank 3)

__attribute__((noinline)) void touch(volatile unsigned char *p) { *p = 1; }

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
    out = arrB1[i] + arrB2[i] + arrB3[i];                // 0x66: FSR+IRP reads
    arrB1[1] = 0x07;
    arrB2[5] = arrB1[1];                                 // banked direct copy (BANKSEL)
    out = (unsigned char)(out + arrB2[5]);               // 0x66 + 0x07 = 0x6D
    struct P g;                                          // alloca; sret target
    g = mk();                                            // sret call into the frame
    out = (unsigned char)(out + g.a + g.b);              // 0x6D + 5 + 6 = 0x78
    touch(&arrB3[1]);
    arrB3[arrB2[2]] = 0x40;                              // chained dynamic index
    out = (unsigned char)(out + arrB3[0]);               // 0x78 + 0x40 = 0xB8
}
