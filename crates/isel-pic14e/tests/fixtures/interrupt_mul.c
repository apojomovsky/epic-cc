// Issue #2 acceptance: main and the ISR both multiply, so both contexts
// reach the injected `__mul_u8` runtime routine.
//
// Before the routine-duplication fix there was exactly ONE `__mul_u8`, and
// its frame (params `a`/`b` plus the 6-byte `__scr` working area) was shared
// by both contexts. An interrupt taken while main was partway through the
// shift-add loop re-entered that same frame, overwrote the multiplier,
// counter and running product, and main resumed against the ISR's state.
// Nothing diagnosed it: the program compiled, ran, and returned a wrong
// number.
//
// The fix gives the ISR context its own `__mul_u8_isr` copy with its own
// slots. The e2e asserts the two frames are disjoint, which is what makes
// the clobber impossible; it does not fire an interrupt mid-routine,
// because `Pic14::fire_interrupt` currently pushes `pc + 1` and so drops
// the instruction at the injection point (tracked separately, issue #15).
//
// Shapes chosen so clang -O1 keeps both multiplies:
//   - every input and output is volatile, so no store is dead
//   - the operands are runtime loads, so neither multiply folds to a
//     constant, and both are byte-wide so both land on __mul_u8
//
// Expected with no interrupt (in_a = 47, in_b = 5). The operands are chosen
// so the quotient and the remainder differ, which is what makes the
// recipe-selection assertion above meaningful:
//   out   = (unsigned char)(47 * 5)  = 235  (0xEB)
//   out_q = 47 / (5 | 1) = 47 / 5    = 9    (the remainder would be 2)
//   isr_out / isr_out_q untouched    = 0
// Both contexts also divide, not just multiply. `__udiv_u8` and `__urem_u8`
// share one recipe and pick quotient vs remainder by name, so the ISR copy
// exercises the recipe-selection path: a copy that matched on its own
// `__udiv_u8_isr` name would fall through to the remainder store and return
// the wrong number.
volatile unsigned char in_a;
volatile unsigned char in_b;
volatile unsigned char out;
volatile unsigned char out_q;

volatile unsigned char isr_a;
volatile unsigned char isr_b;
volatile unsigned char isr_out;
volatile unsigned char isr_out_q;

__attribute__((interrupt(0))) void isr(void) {
    isr_out = (unsigned char)(isr_a * isr_b);
    isr_out_q = (unsigned char)(isr_a / (unsigned char)(isr_b | 1));
}

void main(void) {
    out = (unsigned char)(in_a * in_b);
    out_q = (unsigned char)(in_a / (unsigned char)(in_b | 1));
}
