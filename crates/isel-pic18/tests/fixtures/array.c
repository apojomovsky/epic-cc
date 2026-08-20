// Milestone-5 RAM-array probe: a non-const array written and read at a
// runtime index, the pure FSR/INDF path, no const involvement. `in` is a
// 16-bit volatile so clang keeps the index mask `& 7` as an i16 `and` (isel
// lowers i16 and; it has no i8 and). Expected: in = 3 -> buf[3] = 4 -> out =
// 4.
volatile unsigned short in;
volatile unsigned char out;
volatile unsigned char buf[8];
void main(void) {
    unsigned char i = (unsigned char)(in & 7);
    buf[i] = (unsigned char)(i + 1);
    out = buf[i];
}
