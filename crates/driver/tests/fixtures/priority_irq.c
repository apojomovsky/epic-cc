// Priority-interrupt acceptance (epic-cc#346): a high (`__interrupt(1)`)
// and a low (`__interrupt(2)`) handler sharing the noinline helper
// `bump()` with main and each other. The e2e (priority_irq_e2e.rs) fires
// the low ISR mid-main, then fires high while the low handler is live:
//   - `hi_saw_lo == 1` proves the high ISR ran while low was live
//     (nesting, not sequential service),
//   - `ticks == 2` / `pkts == 2` prove both handlers' multi-call bodies
//     computed correctly under preemption (disjoint frames),
//   - `main_ctr == 3` proves main's state survived both save/restores.
// All three contexts call `bump`, so legalize emits `bump`, `bump_isr`
// and `bump_isr_high`; `bump()` is add-only (no mul/div), so the ISRs
// stay clear of the runtime routines' scratch.
volatile unsigned char ticks;
volatile unsigned char pkts;
volatile unsigned char main_ctr;
volatile unsigned char lo_flag;
volatile unsigned char hi_saw_lo;

__attribute__((noinline)) unsigned char bump(unsigned char x) { return (unsigned char)(x + 1); }

__interrupt(1) void hi_isr(void) {
    unsigned char a = ticks;
    unsigned char b = bump(a);
    ticks = bump(b);
    hi_saw_lo = lo_flag;
}

__interrupt(2) void lo_isr(void) {
    unsigned char a = pkts;
    lo_flag = 1;
    unsigned char b = bump(a);
    unsigned char c = bump(b);
    pkts = c;
    lo_flag = 0;
}

void main(void) {
    ticks = 0;
    pkts = 0;
    main_ctr = 0;
    lo_flag = 0;
    hi_saw_lo = 0;
    while (main_ctr < 3) {
        main_ctr = bump(main_ctr);
    }
}
