// Unions acceptance (PIC14 parity, #268): a union of u8[2]/u16, written
// through one member and read through the other, both directions. Compiles
// through the whole driver pipeline and runs correctly in the simulator.
//
// in = 0x1234: v.w = in, out = v.b[0] + v.b[1] = 0x34 + 0x12 = 0x46 (little
// endian). Then v.b[0] = 0x34, v.b[1] = 0x12, out2 = v.w & 0xFF = 0x34.
//
// `in` carries its input as a real initializer (0x1234): __start clears
// zero-initialized globals before main (epic-cc#561), so an uninitialized
// global can no longer double as a harness-input channel.
volatile unsigned short in = 0x1234;
volatile unsigned char out;
volatile unsigned char out2;
union u { unsigned char b[2]; unsigned short w; };
void main(void) {
    union u v;
    v.w = in;
    out = (unsigned char)(v.b[0] + v.b[1]);
    v.b[0] = 0x34; v.b[1] = 0x12;
    out2 = (unsigned char)(v.w & 0xFF);
}
