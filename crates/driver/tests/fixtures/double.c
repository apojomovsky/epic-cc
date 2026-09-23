// double acceptance: `double` is 32-bit on msp430 (same as float), so
// clang emits `double` as f32-width IR. This fixture exercises the double
// type mapping end-to-end: a double global, double arithmetic (fadd/fmul),
// and a double->unsigned int conversion. Expected (in = 2):
//   a = 1.5, b = (double)in = 2.0, c = a * b = 3.0
//   out = (unsigned int)c & 0xFF = 3
//
// `in` carries its input as a real initializer (2.0f, 0x40000000): __start
// clears zero-initialized globals before main (epic-cc#561), so an
// uninitialized global can no longer double as a harness-input channel.
volatile double in = 2.0;
volatile unsigned char out;

void main(void) {
    double a = 1.5;
    double b = (double)in;
    double c = a * b;
    out = (unsigned char)((unsigned int)c & 0xFF);
}
