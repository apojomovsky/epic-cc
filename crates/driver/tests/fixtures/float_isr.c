// Float-preemption acceptance (epic-cc#357): main does a long float
// computation and a high-priority ISR fires mid-op, itself doing float
// math. Every context gets its own float-routine frames (per-context
// placement), so main's in-flight operands/scratch survive the ISR's
// float work; main resumes and completes with the bit-exact result the
// un-preempted run produces.
//
// The sim fires the ISR on a fixed step cadence gated on GIEH (see
// float_isr_e2e.rs): the float recipes run hundreds of instructions, so
// of the 8 fires several land INSIDE main's in-flight mul/div. `fire`
// exists so the ISR can acknowledge each firing (no re-entry).
volatile float in1;
volatile float in2;
volatile float out;

volatile unsigned char fire;
volatile unsigned char isr_ctr;
volatile float isr_acc;

void __interrupt(1) hi_isr(void) {
    // The acknowledge (fire = 0) keeps the sim's step-interval firing
    // honest: one firing per armed window.
    fire = 0;
    isr_ctr = isr_ctr + 1;
    isr_acc = isr_acc + 1.5f;
}

void main(void) {
    in1 = 3.0f;
    in2 = 2.5f;
    isr_ctr = 0;
    isr_acc = 0.0f;
    fire = 0;

    fire = 1;                            // the firing chain starts here
    out = in1 / in2;                     // 1.2
    out = out + in2 * 9.0f;              // + 22.5 -> 23.7
    out = out + 1.0f / in1;              // + 1/3 -> 24.0333... (RNE)
    out = out + in2 * 4.0f;              // + 10 -> 34.0333...
    // Final: 3.0/2.5 + 2.5*9.0 + 1.0/3.0 + 2.5*4.0 = 1.2+22.5+0.333..+10
    // = 34.0333328... = 0x42082222 under RNE.
}
