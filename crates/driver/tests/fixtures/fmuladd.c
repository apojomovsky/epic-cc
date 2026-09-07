// Regression for the `-ffp-contract=off` flag (epic-cc#267 PIC18 printf
// gap). At -O1 clang fuses a runtime `a*b+c` into a single
// `llvm.fmuladd.f64` intrinsic unless contraction is off; legalize does
// not know that intrinsic and panics (unknown intrinsic). Turning it off
// keeps the expression as separate fmul + fadd, which the soft-float
// runtime lowers. Without the flag this program does not compile.
//
// `in` is a volatile runtime double so clang cannot constant-fold the
// mul+add away. Expected (in = 3.0): r = 3.0*3.0 + 0.5 = 9.5f = 0x41180000,
// out = (unsigned char)((int)r & 0xFF) = 9.
volatile float out;
volatile unsigned char out8;
volatile double in;

void main(void) {
    double a = in;                        // runtime 3.0
    double b = in;                        // runtime 3.0
    double r = a * b + 0.5;               // fmul + fadd (contract off)
    out8 = (unsigned char)((int)r & 0xFF);
    out = (float)r;
}
