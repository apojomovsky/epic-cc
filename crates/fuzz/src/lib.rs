//! Seeded C generator + differential runner (PIC driver+sim vs host clang).
//!
//! Task 1 of milestone 14: the generator skeleton and the differential
//! harness. The generator emits a tiny, deterministic C program in the
//! milestone's "discipline" (unsigned-only arithmetic in genuinely
//! explicit-width types — `u8`/`u16`/`u32` from `TYPEDEF_PROLOGUE`, never
//! bare `unsigned long`, which is 64-bit on LP64 hosts — guarded shifts, a
//! volatile `u8` checksum); the differential runner compiles it twice —
//! through the PIC8 driver into `pic14-sim`, and through host clang into a
//! native binary — seeds the volatile inputs identically on both sides, and
//! compares the resulting checksums.
//!
//! The harness contracts (see docs/27-phase6-random-testing-plan.md):
//! - `generate(seed)` is deterministic (seeded RNG, no entropy);
//! - the C discipline keeps host and PIC semantics identical, so a checksum
//!   mismatch (or a non-halting sim, or a compiler panic) is a real bug;
//! - the PIC side mirrors `crates/driver/tests/long_e2e.rs`: the volatile
//!   globals' addresses come from the same alloc layout the driver used, the
//!   driver binary (a workspace member) produces the hex, `pic14-sim` runs
//!   it, and the machine must halt;
//! - the host side compiles `prog.c` (+ a generated `host_main.c` that seeds
//!   the inputs by name) with the nix shell's `clang` (the pinned clang
//!   WITHOUT `-target`; the unwrapped `$PIC8_CLANG_UNWRAPPED` cannot find the
//!   host's stdio.h) and reads the printed checksum.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The volatile checksum global's name (fixed by the C discipline).
pub const CHECKSUM_NAME: &str = "checksum";

/// The explicit-width typedef prologue emitted at the top of every generated
/// program.
///
/// WHY (the milestone's "Important" fix): Task 1 documented `unsigned long`
/// as "32-bit on both msp430 and the host" — that equivalence is FALSE. On
/// LP64 hosts `unsigned long` is 64-bit, so a u32 computation whose result
/// exceeds 2^32 (e.g. `x * x` for x = 0xFFFFFFFF, or a 64-bit quotient)
/// diverges: msp430 wraps at 2^32, the host does not. `stdint.h` was the
/// first choice (`uint8_t`/`uint16_t`/`uint32_t` are exactly this), but it
/// does NOT resolve under the driver's fixed flags — `-nostdinc` drops the
/// builtin resource-dir include path (verified empirically; adding an
/// explicit `-isystem` fixes it, but the driver's flags are not ours to
/// change). So the robust option is self-contained typedefs guarded on the
/// target macro clang defines for the msp430 triple:
///
/// - u8  = unsigned char  (8 bits on both targets)
/// - u16 = unsigned short (16 bits on both targets)
/// - u32 = msp430: `unsigned long` (msp430 int is 16-bit, so its 32-bit
///        type is long) / host: `unsigned int` (32-bit on the pinned
///        x86-64-linux host) — genuinely 32-bit on BOTH sides.
///
/// With these, u8/u16/u32 arithmetic wraps identically on both sides and
/// the differential is meaningful for values beyond 2^16 (pinned by
/// `u32_arithmetic_wraps_identically_on_both_sides` in tests/differential.rs
/// and by `unsigned_long_u32_arithmetic_mismatches`, which shows the old
/// discipline failing).
pub const TYPEDEF_PROLOGUE: &str = "\
#ifdef __MSP430__\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned long u32;\n\
#else\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned int u32;\n\
#endif\n";

/// The volatile input globals' name prefix (`in0`, `in1`, …).
const INPUT_PREFIX: &str = "in";

/// The fixed input widths, in declaration order (`in0` u8, `in1` u16,
/// `in2` u32).
const INPUT_WIDTHS: [u8; 3] = [8, 16, 32];

/// Sim step budget per differential run (the long e2e uses the same).
const MAX_SIM_STEPS: usize = 5_000_000;

// ---------------------------------------------------------------------------
// Seeded RNG
// ---------------------------------------------------------------------------

/// SplitMix64 — a small, self-contained, deterministic 64-bit PRNG.
///
/// Chosen over a bare LCG at the same zero-dependency cost: SplitMix64 is a
/// few lines, keeps only a 64-bit word of state, and — unlike an LCG, whose
/// low bits cycle visibly — mixes every output bit, so adjacent seeds produce
/// meaningfully different programs while the output stays perfectly
/// reproducible (the corpus contract; no entropy is ever consulted).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` for small `n` (modulo bias is irrelevant here).
    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n)) as u32
    }
}

// ---------------------------------------------------------------------------
// Program model
// ---------------------------------------------------------------------------

/// One volatile input global: `volatile unsigned <width> <name>;`, seeded
/// with `value` (the low `width` bits) on both sides of the differential.
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub value: u32,
    pub width: u8, // 8 | 16 | 32
}

/// A generated program: the C source plus the metadata the differential
/// harness needs to seed and observe it.
#[derive(Debug, Clone)]
pub struct Program {
    pub c_source: String,
    pub inputs: Vec<Input>,
    pub checksum_name: String,
}

// ---------------------------------------------------------------------------
// Generator (Task 2: the full surface + the fixed corpus)
// ---------------------------------------------------------------------------

/// A scalar binary op the generator can emit, with its width guard.
#[derive(Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

/// A generated noinline helper's signature.
struct Helper {
    name: String,
    params: Vec<u8>, // param widths, 0..=3 entries (8 | 16 | 32)
}

/// The in-progress generation state (deterministic: every random choice
/// comes from `rng`, in a fixed order, so `generate(seed)` reproduces).
struct Gen {
    rng: SplitMix64,
    /// `(name, width)` of every scalar local emitted so far (t0, t1, …).
    /// Statements reference only the most recent live ones, keeping main's
    /// live set — and therefore its frame, which the runtime routines'
    /// bank-0 slots must fit under — small (the long_e2e budget: ≤ 9 i32
    /// locals).
    locals: Vec<(String, u8)>,
    /// `(start, end)` index ranges of locals that died with their C block
    /// (if/else arms, loop bodies) — out of scope for later statements.
    dead: Vec<(usize, usize)>,
    /// The generated `main` body statements, one per line where possible
    /// (a flat, structurally-known shape for the Task-3 reducer).
    body: Vec<String>,
    /// The noinline fold helpers (`fold16`/`fold32`) the body needs.
    helpers_src: String,
    used_fold16: bool,
    used_fold32: bool,
    /// Whether the array/struct globals are actually used (declared only
    /// then — every unused global byte costs main's frame budget).
    used_array: bool,
    used_struct: bool,
    /// Estimated main-frame bytes emitted so far (see `frame_budget`).
    frame_est: u32,
    /// The biggest runtime-routine frame (bytes) the program needs so far.
    worst_routine: u32,
    /// True while the feature-flagged (forced) statements are being emitted
    /// (the flag-guaranteed phase): structured statements pick their
    /// cheapest width so every flagged construct fits the frame budget.
    forced: bool,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Gen {
            rng: SplitMix64::new(seed),
            locals: Vec::new(),
            dead: Vec::new(),
            body: Vec::new(),
            helpers_src: String::new(),
            used_fold16: false,
            used_fold32: false,
            used_array: false,
            used_struct: false,
            frame_est: 0,
            worst_routine: 0,
            forced: false,
        }
    }

    fn below(&mut self, n: u32) -> u32 {
        self.rng.below(n)
    }

    fn pick_width(&mut self) -> u8 {
        [8u8, 16, 32][self.below(3) as usize]
    }

    fn new_local(&mut self, w: u8) -> String {
        let name = format!("t{}", self.locals.len());
        self.locals.push((name.clone(), w));
        name
    }

    /// A `(width)`-cast operand: an input, a recent local, or a constant
    /// (always inside `width`'s range — constants never truncate).
    ///
    /// Only SAME-WIDTH inputs/locals are drawn: a cross-width cast would
    /// make clang materialize a zext/trunc def in main's frame, and the
    /// frame-budget model counts the statement's own defs, not the casts —
    /// the corpus found the resulting bank-0 overflow (seeds 34/169/176).
    fn operand(&mut self, w: u8) -> String {
        let ct = ctype(w);
        let roll = self.below(10);
        if roll < 4 {
            let i = self.below(3) as usize;
            if INPUT_WIDTHS[i] == w {
                return format!("({ct}){INPUT_PREFIX}{i}");
            }
        }
        if roll < 7 {
            if let Some(o) = self.recent_local_width(w) {
                return o;
            }
        }
        let v = match w {
            8 => self.rng.next_u64() as u8 as u32,
            16 => self.rng.next_u64() as u16 as u32,
            _ => self.rng.next_u64() as u32,
        };
        format!("{v}u")
    }

    /// An operand that is NEVER a constant (inputs/locals only). Used for
    /// i8/i16 division/modulo divisors: clang strength-reduces a CONSTANT
    /// divisor into a magic-number multiply in i9/i17 arithmetic, which the
    /// IR pipeline cannot parse (found by the corpus at seed 2). u32
    /// constant divisors stay legal (clang emits a plain `udiv i32`). Like
    /// `operand`, only same-width sources (no cast defs in main's frame).
    fn operand_reg(&mut self, w: u8) -> String {
        let ct = ctype(w);
        // The input of the same width (each width has exactly one input:
        // in0 u8, in1 u16, in2 u32).
        let same_input = format!("({ct}){INPUT_PREFIX}{}", w / 8 - 1);
        if self.below(2) == 0 {
            same_input
        } else if let Some(o) = self.recent_local_width(w) {
            o
        } else {
            same_input
        }
    }

    /// The 1st/2nd live local back (skipping block-dead ones), if any, of
    /// EXACTLY width `w` (a different-width local would need a cast def).
    fn recent_local_width(&mut self, w: u8) -> Option<String> {
        let ct = ctype(w);
        if self.locals.is_empty() {
            return None;
        }
        let want = 1 + self.below(2) as usize;
        let mut seen = 0usize;
        for k in 1..=self.locals.len() {
            let idx = self.locals.len() - k;
            if self.dead.iter().any(|&(s, e)| idx >= s && idx < e) {
                continue;
            }
            let (name, lw) = &self.locals[idx];
            if *lw != w {
                continue;
            }
            seen += 1;
            if seen == want {
                return Some(format!("({ct}){name}"));
            }
        }
        None
    }

    /// A comparison condition for `if` (width-explicit, unsigned).
    fn condition(&mut self) -> String {
        let w = self.pick_width();
        let ct = ctype(w);
        let rel = ["<", "<=", ">", ">=", "==", "!="][self.below(6) as usize];
        let a = self.operand(w);
        let b = self.operand(w);
        format!("(({ct}){a} {rel} ({ct}){b})")
    }

    /// The checksum fold expression for a `width`-bit value: every byte of
    /// the value mixes in (via explicit casts), so a miscompile in ANY byte
    /// of a u16/u32 value changes the checksum. This is the BODY of the
    /// noinline `fold16`/`fold32` helpers — its shift/xor defs live in the
    /// helpers' frames (which have no bank constraint), NOT in main's
    /// (whose frame the runtime routines' bank-0 slots must fit under; the
    /// whole-program backend gives every SSA def its own RAM slot, so the
    /// byte-mix of a u32 would otherwise cost ~27 bytes of main frame).
    fn fold_expr(w: u8, v: &str) -> String {
        match w {
            8 => format!("(u8){v}"),
            16 => format!("(u8)((u8){v} ^ (u8)(((u16){v}) >> 8u))"),
            _ => format!(
                "(u8)((u8){v} ^ (u8)(((u32){v}) >> 8u) ^ (u8)(((u32){v}) >> 16u) ^ (u8)(((u32){v}) >> 24u))"
            ),
        }
    }

    /// The volatile fold store after every statement pins the statement's
    /// ops in the IR on both sides and keeps live values to the
    /// just-computed one. u8 folds inline (1 xor); u16/u32 fold through the
    /// noinline `fold16`/`fold32` helpers so the byte-mix defs stay out of
    /// main's frame (the call itself also exercises the call/ret surface).
    fn push_fold(&mut self, w: u8, v: &str) {
        let line = self.fold_line(w, v);
        self.body.push(line);
    }

    /// The fold statement line for a value, marking the fold helpers used.
    fn fold_line(&mut self, w: u8, v: &str) -> String {
        match w {
            8 => format!("  checksum = (u8)(checksum ^ (u8){v});"),
            16 => {
                self.used_fold16 = true;
                format!("  checksum = (u8)(checksum ^ fold16({v}));")
            }
            _ => {
                self.used_fold32 = true;
                format!("  checksum = (u8)(checksum ^ fold32({v}));")
            }
        }
    }

    /// The backend gives every SSA def (volatile loads included) its own
    /// RAM slot, so main's frame size = the sum of its defs' widths. The
    /// runtime routines are main's callees: their frames start at main's
    /// frame end and their LAST slot must stay before the common-RAM jump
    /// at 0x70 (the loud isel bank-0 assert; 0x70-0x7F is never used by
    /// locals), so main's frame is capped by the biggest routine the
    /// program uses. Measured routine frames (params + scratch):
    ///   u8 mul/div/rem/shift: 3, u16 shift: 6, u16 div/rem: 8,
    ///   u32 shift: 12, u32 div/rem: 12, u16 mul: 18 (14-byte scratch),
    ///   u32 mul: 22.
    ///
    /// Globals end (measured from the allocator): the fixed inputs
    /// (in0 u8 @0x20, in1 u16 @0x22, in2 u32 @0x24) + checksum u8 end at
    /// 0x29; `arr[8]` adds 8, the struct (u8 a / u16 b / u32 c, even-
    /// aligned) adds 8. The old 0x28/6-byte-struct estimate ran low by up
    /// to 4 bytes when both globals were used, silently eating into the
    /// bank-0 headroom the model thinks it has.
    fn frame_budget(&self) -> u32 {
        let globals = 0x29
            + if self.used_array { 8 } else { 0 }
            + if self.used_struct { 8 } else { 0 };
        0x70 - self.worst_routine - globals
    }

    /// Does a statement costing `frame` main bytes and requiring a
    /// `routine`-byte runtime frame still fit? (`uses_array`/`uses_struct`
    /// are the post-statement globals.)
    fn fit(&self, frame: u32, routine: u32, uses_array: bool, uses_struct: bool) -> bool {
        let globals = 0x29
            + if self.used_array || uses_array { 8 } else { 0 }
            + if self.used_struct || uses_struct { 8 } else { 0 };
        let routine = self.worst_routine.max(routine);
        // The 8-byte safety margin applies to the FILL phase only: the
        // per-statement estimates are measured upper bounds (>= real), so
        // forced statements must fit by estimate alone — the margin would
        // reject real-fit flagged combos (e.g. array+struct: 35 est of a
        // 41 budget). The fill statements' cumulative real cost is still
        // bounded by est + 8 <= the hard bank-0 limit.
        let margin = if self.forced { 0 } else { 8 };
        self.frame_est + frame + margin <= 0x70 - routine - globals
    }

    /// The noinline byte-mix fold helpers (emitted only when used).
    fn fold_helpers_src(used16: bool, used32: bool) -> String {
        let mut s = String::new();
        // The trailing `+ (u8)in0` is a volatile read: without it the body
        // is pure arithmetic on the arg, so a constant-foldable arg makes
        // clang specialize the helper and dead-arg the original call into
        // `poison` (seed 2 — the IR pipeline cannot parse poison). in0 is
        // seeded identically on both sides, so the fold stays deterministic.
        if used16 {
            s.push_str(
                "__attribute__((noinline)) u8 fold16(u16 v) {\n    return (u8)((u8)v ^ (u8)(v >> 8u) + (u8)in0);\n}\n",
            );
        }
        if used32 {
            s.push_str(
                "__attribute__((noinline)) u8 fold32(u32 v) {\n    return (u8)((u8)v ^ (u8)(v >> 8u) ^ (u8)(v >> 16u) ^ (u8)(v >> 24u) + (u8)in0);\n}\n",
            );
        }
        s
    }

    /// Emit `{ct} tK = expr;` (declare-and-initialize — every generated
    /// local is single-assignment) + the fold, and return the local name.
    fn push_compute(&mut self, w: u8, expr: String) -> String {
        let t = self.new_local(w);
        self.body.push(format!("  {ct} {t} = {expr};", ct = ctype(w)));
        self.push_fold(w, &t);
        t
    }

    /// A scalar arithmetic statement at a random width and op, with the
    /// discipline's guards: shifts by a const < width (or a masked count),
    /// divisors forced nonzero (`| 1u`), mul computed in the next wider
    /// space so the host's promotion to `int` cannot overflow (u8/u16
    /// values truncate back identically on both sides).
    ///
    /// Width/op budget: the whole-program backend gives every SSA def its
    /// own RAM slot, so main's frame (and the runtime routines' bank-0
    /// slots derived from it) caps how many expensive defs a program may
    /// hold. u8 ops are cheapest; u16/u32 byte-mix folds go through the
    /// fold helpers; u32 mul/div/rem (the big runtime routines) are a
    /// deliberate minority.
    fn emit_arith(&mut self) -> bool {
        self.emit_arith_inner(None)
    }

    /// Emit a specific op (the feature-flag guarantees); `None` picks by
    /// the weighted mix.
    fn emit_forced_arith(&mut self, op: BinOp) -> bool {
        self.emit_arith_inner(Some(op))
    }

    fn emit_arith_inner(&mut self, forced: Option<BinOp>) -> bool {
        let r = self.below(100);
        let mut w = if r < 50 { 8 } else if r < 80 { 16 } else { 32 };
        let op = match forced {
            Some(op) => op,
            None if w == 32 => match self.below(10) {
                0..=1 => BinOp::Mul,
                2 => BinOp::Div,
                3 => BinOp::Rem,
                4 => BinOp::Shl,
                5 => BinOp::Shr,
                _ => [BinOp::Add, BinOp::Sub, BinOp::And, BinOp::Or, BinOp::Xor]
                    [self.below(5) as usize],
            },
            None => {
                // Div/Rem/Mul/Shl/Shr weighted up so the corpus reliably
                // exercises the whole op surface (the frame budget rejects
                // the expensive ones often enough on its own). Add/Sub
                // stay covered by the if/loop/helper bodies. Shl/Shr get
                // 5 of the 10 slots: the 8 fast seeds must jointly
                // exercise every op and the 200-seed corpus needs >= 40
                // shifts (the pinned coverage sanity checks).
                match self.below(10) {
                    0 => BinOp::Div,
                    1..=2 => BinOp::Rem,
                    3..=4 => BinOp::Mul,
                    5..=7 => BinOp::Shl,
                    _ => BinOp::Shr,
                }
            }
        };
        // A FORCED (flag-guaranteed) op must fit, and the later forced
        // statements' globals (array/struct) shrink the budget, so a heavy
        // forced op always runs at u8 — the cheapest width. The guarantee
        // is on the OP, not the width: a u8 mul still pulls in the mul
        // runtime routine (mul i16), and u16/u32 arith stays covered by
        // the fill statements. Fill statements keep the random width and
        // simply return false when the budget rejects them (best-effort).
        if forced.is_some() {
            w = 8;
        }
        // (main-frame cost, runtime-routine frame the statement needs).
        // Note w=8 mul lowers as mul i16 (__mul_u16, 18 bytes) and w=16 mul
        // as mul i32 (__mul_u32, 22 bytes): the width-space widening for
        // host-overflow safety pulls in the big routines.
        let (cost, routine) = arith_cost(w, op);
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        let a = self.operand(w);
        // i8/i16 div/rem need a RUNTIME divisor (a constant divisor makes
        // clang strength-reduce into magic-number i9/i17 multiplies the IR
        // pipeline cannot parse; u32 keeps `udiv i32` — see `operand_reg`).
        let b = if w < 32 && matches!(op, BinOp::Div | BinOp::Rem) {
            self.operand_reg(w)
        } else {
            self.operand(w)
        };
        let expr = match op {
            BinOp::Shl | BinOp::Shr => {
                // Shift count: const < width, or a masked runtime count.
                let r = self.below(3);
                let (l, r_op) = (format!("({}){a}", ctype(w)), format!("({}){b}", ctype(w)));
                if r < 2 {
                    let c = self.below(u32::from(w));
                    format!("({ct})({l} {s} {c}u)", ct = ctype(w), s = if matches!(op, BinOp::Shl) { "<<" } else { ">>" })
                } else {
                    let m = w - 1;
                    format!(
                        "({ct})({l} {s} ({ct})(({ct}){r_op} & {m}u))",
                        ct = ctype(w),
                        s = if matches!(op, BinOp::Shl) { "<<" } else { ">>" }
                    )
                }
            }
            _ => {
                let s = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Rem => "%",
                    BinOp::And => "&",
                    BinOp::Or => "|",
                    BinOp::Xor => "^",
                    _ => unreachable!(),
                };
                // The width-space choice: + - & | ^ at w (promotion is
                // value-safe); * and / % widen so the host's int promotion
                // cannot overflow and divisors stay nonzero.
                let (wa, wb, guard) = match (op, w) {
                    (BinOp::Mul, 8) => (16u8, 16u8, ""),
                    (BinOp::Mul, 16) => (32, 32, ""),
                    (BinOp::Mul, _) => (32, 32, ""),
                    (BinOp::Div | BinOp::Rem, 8) => (16, 16, "| 1u"),
                    (BinOp::Div | BinOp::Rem, 16) => (16, 16, "| 1u"),
                    (BinOp::Div | BinOp::Rem, _) => (32, 32, "| 1u"),
                    _ => (w, w, ""),
                };
                format!(
                    "({ct})(({twa}){a} {s} (({twb}){b}{guard}))",
                    ct = ctype(w),
                    twa = ctype(wa),
                    twb = ctype(wb)
                )
            }
        };
        self.push_compute(w, expr);
        true
    }

    /// A comparison statement: the i1 result stored as u8 and folded.
    fn emit_cmp(&mut self) -> bool {
        // Main-frame cost = the two operands + the i1 result, MEASURED
        // from clang -O1 IR: u8 6, u16 ~8, u32 12 bytes of defs (rounded
        // up; the flat-7 estimate under-ran u32 by 5).
        let w = self.pick_width();
        let cost = match w {
            8 => 7,
            16 => 9,
            _ => 13,
        };
        if !self.fit(cost, 0, false, false) {
            return false;
        }
        self.frame_est += cost;
        let ct = ctype(w);
        let rel = ["<", "<=", ">", ">=", "==", "!="][self.below(6) as usize];
        let a = self.operand(w);
        let b = self.operand(w);
        self.push_compute(8, format!("(u8)(({ct}){a} {rel} ({ct}){b})"));
        true
    }

    /// An if/else: both branches compute a width-w value and fold it into
    /// the checksum (branch-conditional folding; the same seeded inputs run
    /// the same branch on both sides, and both arms' code survives).
    fn emit_ifelse(&mut self) -> bool {
        // A forced (flag-guaranteed) if always runs at u8 — the cheapest —
        // so it fits alongside the other forced statements; the fill phase
        // keeps the random width (u32 ifs are expensive, ~30 main bytes).
        let w = if self.forced { 8 } else { self.pick_width() };
        let ct = ctype(w);
        // Main-frame cost = the condition's operands/result + one local
        // per arm, MEASURED from clang -O1 IR: u8 ~8-9, u16 ~16, u32 ~30
        // bytes of defs.
        let cost = match w {
            8 => 9,
            16 => 16,
            _ => 30,
        };
        if !self.fit(cost, 0, false, false) {
            return false;
        }
        self.frame_est += cost;
        let cond = self.condition();
        let arm = |g: &mut Self| -> String {
            let op = ["+", "-", "^", "|", "&"][g.below(5) as usize];
            let a = g.operand(w);
            let b = g.operand(w);
            let v = match op {
                "-" => format!("({ct})(({ct}){a} - ({ct}){b})"),
                _ => format!("({ct})(({ct}){a} {op} ({ct}){b})"),
            };
            let t = g.new_local(w);
            let line = format!("    {ct} {t} = {v};");
            let fold = g.fold_line(w, &t).replace("  ", "    ");
            format!("{line}\n{fold}")
        };
        // The then-arm's locals die at the end of their block — mark them
        // dead BEFORE generating the else arm, so the else arm (and later
        // statements) can never reference them.
        let then = arm(self);
        self.dead.push((self.locals.len() - 1, self.locals.len()));
        let els = arm(self);
        self.body.push(format!("  if ({cond}) {{\n{then}\n  }} else {{\n{els}\n  }}"));
        self.dead.push((self.locals.len() - 1, self.locals.len()));
        true
    }

    /// A bounded loop: `for (i = 0; i < n; i++)` with n <= 8 (a masked
    /// input), body = 1–2 cheap inline ops on the accumulator (no runtime
    /// routines inside the trip loop), then fold the accumulator.
    fn emit_loop(&mut self) -> bool {
        // Bias the accumulator to u8 (a u32 loop's phi web costs ~4x a u8
        // one in main's frame and starves the mul/div statements of budget
        // — u32 math is covered by arith/struct/cmp instead). A forced
        // (flag-guaranteed) loop always runs at u8.
        let w = if self.forced {
            8
        } else {
            [8u8, 8, 8, 16][self.below(4) as usize]
        };
        let ct = ctype(w);
        // Main-frame cost = i + n + acc + t + 1-2 body temps, MEASURED
        // from clang -O1 IR: u8 11, u16 ~15 bytes of defs (rounded up).
        let cost = match w {
            8 => 12,
            _ => 16,
        };
        // The `in0 % 5u` bound calls __urem_u8 (3 bytes); a variable-shift
        // body op calls __shl/__lshr at the accumulator width.
        let routine = [3u32, 6][match w {
            8 => 0,
            _ => 1,
        }]
        .max(3);
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        // Bounds: a mask or a constant (a const `%` on i8/i16 becomes a
        // magic-number i9/i17 multiply the IR pipeline cannot parse).
        let n_expr = match self.below(3) {
            0 => "(u8)(in0 & 7u)".to_string(),
            1 => format!("{}u", 2 + self.below(4)),
            _ => format!("{}u", 2 + self.below(4)),
        };
        let mut body = String::new();
        // 1-2 body ops (each is a def in the phi web — keep the loop cheap).
        // NO `+`/`-` on the induction var: clang -O1 strength-reduces
        // `acc += i` over the masked bound into a closed-form sum in i9
        // magic arithmetic, which the IR pipeline cannot parse (found by
        // the corpus at seed 78). `^`/`&`/`|` are not sum idioms, so the
        // loop stays a real loop.
        let nops = 1 + self.below(2);
        for _ in 0..nops {
            let op = ["^", "&", "|"][self.below(3) as usize];
            body.push_str(&format!(
                "    acc = ({ct})(({ct})acc {op} ({ct})i);\n"
            ));
        }
        if self.below(2) == 0 {
            // A masked variable shift (count < width) inside the loop.
            let m = w - 1;
            let s = if self.below(2) == 0 { "<<" } else { ">>" };
            body.push_str(&format!(
                "    acc = ({ct})(({ct})acc {s} ({ct})(({ct})i & {m}u));\n"
            ));
        }
        let m = self.locals.len();
        let t = self.new_local(w);
        let fold = self.fold_line(w, &t).replace("  ", "    ");
        let block = format!(
            "  {{\n    u8 i;\n    u8 n = {n_expr};\n    {ct} acc = 0u;\n    for (i = 0u; i < n; i++) {{\n{body}    }}\n    {ct} {t} = acc;\n{fold}\n  }}"
        );
        self.body.push(block);
        self.dead.push((m, self.locals.len()));
        true
    }

    /// A noinline call: `t = helper(args);` (0–3 unsigned params), folded.
    fn emit_call(&mut self, helpers: &[Helper]) -> bool {
        // Main-frame cost = the single u8 result local (+ fold), MEASURED
        // at 3 defs from clang -O1 IR.
        if !self.fit(5, 0, false, false) {
            return false;
        }
        self.frame_est += 5;
        let h = &helpers[self.below(helpers.len() as u32) as usize];
        // Constant args only: clang -O1 was observed replacing a
        // volatile-derived call arg (a zext of a loaded value, local or
        // input) with `poison` (seed 2 of the corpus), which the IR
        // pipeline cannot parse. Constant args are always clean and still
        // exercise the full call/param/return machinery.
        let args: Vec<String> = h
            .params
            .iter()
            .map(|&pw| {
                let v = match pw {
                    8 => self.rng.next_u64() as u8 as u32,
                    16 => self.rng.next_u64() as u16 as u32,
                    _ => self.rng.next_u64() as u32,
                };
                format!("{v}u")
            })
            .collect();
        let t = self.new_local(8);
        self.body
            .push(format!("  u8 {t} = {}({});", h.name, args.join(", ")));
        self.push_fold(8, &t);
        true
    }

    /// An array statement: a dynamic in-bounds index `i % N` (N small;
    /// power-of-two N lowers to a mask, 3/5 to a real urem), a write, a
    /// read-back folded into the checksum.
    fn emit_array(&mut self) -> bool {
        // Main-frame cost = the x/y operands + the ix local + the index
        // casts, MEASURED at 8-10 defs from clang -O1 IR (seed 169: the
        // index zext pushes it to 10). `i % 3`/`i % 5` lower to a real
        // __urem_u16 call (8-byte frame); pow2 N lowers to an `and`.
        if !self.fit(11, 14, true, false) {
            return false;
        }
        self.frame_est += 11;
        self.worst_routine = self.worst_routine.max(14);
        self.used_array = true;
        // Power-of-two sizes only: `i % 8u`/`i % 4u` lower to an `and`;
        // a non-pow2 const modulus would strength-reduce into an i17
        // magic multiply (see `operand_reg`). The real urem surface is
        // covered by the runtime-divisor arith statements.
        let n = [8u32, 4, 8, 4][self.below(4) as usize];
        // Operands FIRST — a local created before them would self-reference.
        let x = self.operand(16);
        let y = self.operand(8);
        let ix = self.new_local(8);
        self.body.push(format!("  u8 {ix} = (u8)((u16){x} % {n}u);"));
        self.body.push(format!("  arr[{ix}] = (u8){y};"));
        self.body.push(format!("  checksum = (u8)(checksum ^ (u8)arr[{ix}]);"));
        true
    }

    /// A struct statement: field-wise stores into the volatile global
    /// struct `s` (u8/u16/u32 fields), then a width-mixing fold over the
    /// fields (explicit casts; no layout dependence — names only).
    fn emit_struct(&mut self) -> bool {
        // Main-frame cost = the three operand locals + the field loads +
        // the s.c u32 shift/xor chain, MEASURED at 21-23 defs from clang
        // -O1 IR (the u32 fold is expensive in main's frame — the old
        // 13-byte estimate under-ran by ~10 and overflowed bank 0 once
        // the routine frames were stacked on main's end).
        if !self.fit(24, 0, false, true) {
            return false;
        }
        self.frame_est += 24;
        self.used_struct = true;
        let a = self.operand(8);
        let b = self.operand(16);
        let c = self.operand(32);
        self.body.push(format!("  s.a = (u8){a};"));
        self.body.push(format!("  s.b = (u16){b};"));
        self.body.push(format!("  s.c = (u32){c};"));
        // Field-wise reads folded back with explicit width-mixing casts (the
        // fold "can mix widths": each field folds through its own width).
        let mode = self.below(2);
        let line = if mode == 0 {
            self.used_fold16 = true;
            self.used_fold32 = true;
            "  checksum = (u8)(checksum ^ (u8)s.a ^ fold16((u16)s.b) ^ fold32((u32)s.c));".to_string()
        } else {
            self.used_fold16 = true;
            "  checksum = (u8)(checksum ^ (u8)s.a ^ fold16((u16)s.b) ^ (u8)((u32)s.c >> 24u));"
                .to_string()
        };
        self.body.push(line);
        true
    }

    /// Emit the helper functions (1–3): noinline, 0–3 unsigned params,
    /// u8 return. Bodies use only INLINE ops (add/sub/and/or/xor/const
    /// shifts/icmps — no mul/div/rem, whose runtime-routine frames would
    /// stack on main's frame) so helper frames never hit the bank-0 limit.
    fn emit_helpers(&mut self) -> (Vec<Helper>, String) {
        let n = 1 + self.below(3) as usize; // 1..=3
        let mut helpers = Vec::with_capacity(n);
        let mut src = String::new();
        for k in 0..n {
            let np = self.below(4) as usize; // 0..=3 params
            let params: Vec<u8> = (0..np).map(|_| self.pick_width()).collect();
            let mut sig = String::new();
            for (i, &pw) in params.iter().enumerate() {
                if i > 0 {
                    sig.push_str(", ");
                }
                sig.push_str(&format!("{} p{}", ctype(pw), i));
            }
            let name = format!("helper{k}");
            src.push_str(&format!(
                "__attribute__((noinline)) u8 {name}({sig}) {{\n"
            ));
            let mut prev: Vec<(String, u8)> = Vec::new();
            // One op PER PARAM first: every param must be referenced in the
            // body, or clang replaces the unused call arg with `poison`
            // (found by the corpus at seed 19 — the IR pipeline cannot
            // parse poison, so it panics loudly). The op's second operand is
            // a VOLATILE INPUT read: that makes the body impossible to
            // constant-fold/specialize, so clang cannot dead-arg the call
            // into `poison` either (seen at seed 2, where a foldable helper
            // body was specialized and its original call left with a poison
            // arg).
            for (pi, &pw) in params.iter().enumerate() {
                let ct = ctype(pw);
                let v = format!("v{}", prev.len());
                let i = self.below(3);
                let op = ["+", "-", "&", "|", "^"][self.below(5) as usize];
                src.push_str(&format!(
                    "    {ct} {v} = ({ct})(({ct})p{pi} {op} ({ct}){INPUT_PREFIX}{i});\n"
                ));
                prev.push((v, pw));
            }
            // Then 1-2 extra ops over params/prev locals/constants (inline
            // ops only — no mul/div/rem, whose runtime-routine frames
            // would stack on main's frame; helpers stay self-contained).
            let nops = 1 + self.below(2) as usize;
            for _ in 0..nops {
                let w = self.pick_width();
                let ct = ctype(w);
                let pick = |g: &mut Self, w: u8, prev: &[(String, u8)]| -> String {
                    let roll = g.below(6);
                    if roll < 3 && !params.is_empty() {
                        let pi = g.below(params.len() as u32) as usize;
                        format!("({ct2})p{pi}", ct2 = ctype(params[pi]))
                    } else if roll < 5 && !prev.is_empty() {
                        let (name, _) = &prev[prev.len() - 1];
                        format!("({ct2}){name}", ct2 = ctype(w))
                    } else {
                        let v = match w {
                            8 => g.rng.next_u64() as u8 as u32,
                            16 => g.rng.next_u64() as u16 as u32,
                            _ => g.rng.next_u64() as u32,
                        };
                        format!("{v}u")
                    }
                };
                let a = pick(self, w, &prev);
                let b = pick(self, w, &prev);
                // No shifts inside helpers: clang matches a const-shift
                // followed by the byte-mix return as a rotate idiom and
                // emits `llvm.fshl.i8`, an intrinsic the whole-program
                // compiler cannot resolve (found by the corpus at seed 1).
                let op = ["+", "-", "&", "|", "^"][self.below(5) as usize];
                let expr = format!("({ct})(({ct}){a} {op} ({ct}){b})");
                let v = format!("v{}", prev.len());
                src.push_str(&format!("    {ct} {v} = {expr};\n"));
                prev.push((v, w));
            }
            let (last, _lw) = prev.last().unwrap().clone();
            // The return mixes in a volatile input: a pure byte-mix of the
            // params lets clang collapse the body to `ret %0` (identity
            // folds) and mark the param `returned`, which the IR parser
            // cannot handle (found by the corpus at seed 2). in0 is seeded
            // identically on both sides, so the mix is deterministic.
            src.push_str(&format!("    return (u8)((u8){last} + (u8)in0);\n}}\n"));
            helpers.push(Helper { name, params });
        }
        (helpers, src)
    }
}

fn ctype(w: u8) -> &'static str {
    match w {
        8 => "u8",
        16 => "u16",
        32 => "u32",
        w => panic!("bad width {w}"),
    }
}

/// (main-frame cost, runtime-routine frame) for an arith statement at a
/// width. The main-frame costs are MEASURED from clang -O1 IR for the
/// generated statement shapes (volatile loads + the op + the fold call),
/// rounded up: u8 5, u16 9, u32 15 bytes of defs. Note w=8 mul lowers as
/// mul i16 (__mul_u16, 18 bytes) and w=16 mul as mul i32 (__mul_u32, 22
/// bytes): the width-space widening for host-overflow safety pulls in the
/// big routines.
fn arith_cost(w: u8, op: BinOp) -> (u32, u32) {
    match (w, op) {
        (8, BinOp::Mul) => (8, 18),
        (8, BinOp::Div | BinOp::Rem) => (10, 14),
        (8, BinOp::Shl | BinOp::Shr) => (8, 3), // variable count maybe
        (8, _) => (7, 0),
        (16, BinOp::Mul) => (14, 22),
        (16, BinOp::Div | BinOp::Rem) => (12, 14),
        (16, BinOp::Shl | BinOp::Shr) => (10, 6),
        (16, _) => (10, 0),
        (32, BinOp::Mul) => (16, 22),
        (32, BinOp::Div | BinOp::Rem) => (19, 12),
        (32, BinOp::Shl | BinOp::Shr) => (13, 12),
        (32, _) => (15, 0),
        _ => unreachable!(),
    }
}

/// Generate a deterministic program from `seed`.
///
/// The full Task-2 surface: scalar arithmetic (+ - * / % & | ^ << >> on
/// u8/u16/u32 with the discipline's guards), comparisons (< <= > >= == !=),
/// if/else, bounded loops, noinline calls (0–3 unsigned params), arrays
/// (small, dynamic `i % N` index), structs (simple u8/u16/u32 fields,
/// field-wise access), all folded into the volatile `u8` checksum with
/// explicit width casts. 3 fixed-width volatile inputs (in0 u8, in1 u16,
/// in2 u32) are seeded identically on both sides of the differential. Every
/// random choice comes from the seeded RNG in a fixed order, so `seed`
/// fully determines the program (the corpus contract).
pub fn generate(seed: u64) -> Program {
    let mut g = Gen::new(seed);
    let mut rng = SplitMix64::new(seed ^ 0x51_7C_C1_B7_27_22_0A_95);

    // Per-seed feature flags: each construct/op is guaranteed to appear in
    // a program when its flag is set (the RNG mix varies which programs are
    // rich; the fixed 8-seed fast corpus and the 200-seed corpus span the
    // surface — pinned by the tests' coverage sanity checks). The heavy ops
    // (mul/div/rem pull in the big runtime routines) are flags too: forced
    // FIRST, while the frame budget is empty, so the random fill cannot
    // starve them.
    //
    // The flags are a BOUNDED random subset — exactly 2 of the 8, not
    // independent bits: main's frame (and the runtime routines' bank-0
    // slots derived from it) is a hard hardware limit, so one program can
    // only hold a couple of heavy constructs. An unbounded bit-draw let a
    // seed's forced tail exceed the budget and was SILENTLY DROPPED
    // (review finding — 'guaranteed when flagged' was false); force() now
    // panics if a flagged construct cannot fit, so the draw must stay
    // inside the budget by construction. The RNG mix still varies which
    // programs are rich (which 2 of the 8), and the fill statements cover
    // the rest of the surface.
    let nflags = 2; // exactly 2 flags per seed (see force()'s panic)
    let mut flags = [false; 8];
    let mut pool: Vec<u8> = (0..8).collect();
    for _ in 0..nflags {
        let i = (rng.next_u64() as usize) % pool.len();
        flags[pool.remove(i) as usize] = true;
    }
    let want_mul = flags[0];
    let want_div = flags[1];
    let want_rem = flags[2];
    let want_if = flags[3];
    let want_loop = flags[4];
    let want_call = flags[5];
    let want_array = flags[6];
    let want_struct = flags[7];

    // Inputs: a fixed u8/u16/u32 mix so every program exercises all three
    // widths (and u32 values beyond 2^16 in every run).
    let mut inputs = Vec::new();
    let mut decls = String::new();
    for (i, w) in [(0usize, 8u8), (1, 16), (2, 32)] {
        let name = format!("{INPUT_PREFIX}{i}");
        decls.push_str(&format!("volatile {} {name};\n", ctype(w)));
        let value = match w {
            8 => rng.next_u64() as u8 as u32,
            16 => rng.next_u64() as u16 as u32,
            _ => rng.next_u64() as u32,
        };
        inputs.push(Input { name, value, width: w });
    }
    decls.push_str(&format!("volatile u8 {checksum};\n", checksum = CHECKSUM_NAME));

    let (helpers, helper_src) = g.emit_helpers();

    // Feature-flagged statements FIRST (frame budget empty, so the heavy
    // mul/div/rem and the structured constructs all fit), then a weighted
    // random fill bounded by the frame budget — the backend gives every
    // SSA def, volatile loads included, its own RAM slot, so main's frame,
    // and the runtime routines' bank-0 slots derived from it, cap the
    // program's size. While `forced` is set, the structured statements
    // pick their cheapest width so every flagged construct fits.
    let force = |g: &mut Gen, k: usize| -> bool {
        match k {
            0 => g.emit_forced_arith(BinOp::Mul),
            1 => g.emit_forced_arith(BinOp::Div),
            2 => g.emit_forced_arith(BinOp::Rem),
            3 => g.emit_ifelse(),
            4 => g.emit_loop(),
            5 => g.emit_call(&helpers),
            6 => g.emit_array(),
            _ => g.emit_struct(),
        }
    };
    g.forced = true;
    for (k, want) in [
        want_mul,
        want_div,
        want_rem,
        want_if,
        want_loop,
        want_call,
        want_array,
        want_struct,
    ]
    .iter()
    .enumerate()
    {
        if *want && !force(&mut g, k) {
            // A flagged construct is GUARANTEED to appear (the tests pin
            // the corpus's per-seed feature coverage). Silently dropping it
            // on a budget rejection would lose that coverage without a
            // trace, so fail loudly instead — the frame-budget model must
            // be recalibrated (cheaper forced variants, fewer simultaneous
            // flags) until every flagged construct fits.
            panic!(
                "fuzz: seed {seed}: flagged construct #{k} rejected by the frame budget \
                 (frame_est {}, worst_routine {}, budget {}) — the 'guaranteed when \
                 flagged' contract is broken; recalibrate the budget model",
                g.frame_est,
                g.worst_routine,
                g.frame_budget()
            );
        }
    }
    g.forced = false;
    let stmt_kind = |g: &mut Gen| -> usize {
        // returns 0..4 (if/loop/call/array/struct) or 5/6 (arith/cmp)
        let r = g.below(100);
        match r {
            0..=9 => 0,   // ifelse 10%
            10..=21 => 1, // loop 12%
            22..=33 => 2, // call 12%
            34..=40 => 3, // array 7%
            41..=47 => 4, // struct 7%
            48..=81 => 5, // arith 34%
            _ => 6,       // cmp 18%
        }
    };
    for _ in 0..14 {
        let ok = match stmt_kind(&mut g) {
            0 => g.emit_ifelse(),
            1 => g.emit_loop(),
            2 => g.emit_call(&helpers),
            3 => g.emit_array(),
            4 => g.emit_struct(),
            5 => g.emit_arith(),
            _ => g.emit_cmp(),
        };
        if !ok {
            break; // the frame budget is exhausted
        }
    }

    // The array/struct globals only when used (every global byte costs
    // main's frame budget; the fold helpers only when u16/u32 folds occur).
    if g.used_array {
        decls.push_str("volatile u8 arr[8];\n");
    }
    if g.used_struct {
        decls.push_str("volatile struct S { u8 a; u16 b; u32 c; } s;\n");
    }

    let body = g.body.join("\n");
    let c_source = format!(
        "{TYPEDEF_PROLOGUE}{decls}{helper_src}{fold_src}void main(void) {{\n{body}\n}}\n",
        fold_src = Gen::fold_helpers_src(g.used_fold16, g.used_fold32)
    );
    Program {
        c_source,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Differential runner
// ---------------------------------------------------------------------------

/// Run the program on both sides and return the agreed checksum, or a
/// description of the failure: a compile/driver error (including a compiler
/// panic, which surfaces as a failed process or a caught pipeline panic), a
/// non-halting sim run, or a host/PIC checksum mismatch.
pub fn run_differential(program: &Program) -> Result<u32, String> {
    let dir = WorkDir::new();
    let c_path = dir.path.join("prog.c");
    std::fs::write(&c_path, &program.c_source)
        .map_err(|e| format!("write prog.c: {e}"))?;

    let pic = run_pic(program, &c_path, &dir)?;
    let host = run_host(program, &c_path, &dir)?;

    if pic == host {
        Ok(pic)
    } else {
        Err(format!("mismatch: pic checksum {pic}, host checksum {host}"))
    }
}

/// PIC side: alloc layout (in-process, mirroring the driver's e2e) for the
/// input/checksum addresses, the driver binary for the hex, `pic14-sim`
/// seeded at those addresses, run, checksum read, `halted()` required.
fn run_pic(program: &Program, c_path: &Path, dir: &WorkDir) -> Result<u32, String> {
    let layout = pic_layout(c_path)?;
    let checksum_addr = *layout
        .globals
        .get(&program.checksum_name)
        .ok_or_else(|| format!("no global '{}' in the alloc map", program.checksum_name))?;

    let hex_path = dir.path.join("prog.hex");
    run_driver(c_path, &hex_path)?;

    let hex =
        std::fs::read_to_string(&hex_path).map_err(|e| format!("read {}: {e}", hex_path.display()))?;
    let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
    for input in &program.inputs {
        let addr = *layout
            .globals
            .get(&input.name)
            .ok_or_else(|| format!("no global '{}' in the alloc map", input.name))?;
        seed_le(&mut p, addr, input.width, input.value);
    }
    p.run(MAX_SIM_STEPS);
    if !p.halted() {
        return Err(format!("simulator did not halt within {MAX_SIM_STEPS} steps"));
    }
    Ok(read_le(p.ram(), checksum_addr, 1) as u32)
}

/// Host side: compile `prog.c` + a generated `host_main.c` with host clang
/// (no `-target`), run the native binary, parse the printed checksum.
fn run_host(program: &Program, c_path: &Path, dir: &WorkDir) -> Result<u32, String> {
    let hm_path = dir.path.join("host_main.c");
    std::fs::write(&hm_path, host_main_source(program)?)
        .map_err(|e| format!("write host_main.c: {e}"))?;

    let clang = host_clang();
    let obj_prog = dir.path.join("prog_pic.o");
    let obj_host = dir.path.join("host_main.o");
    let exe = dir.path.join("prog");

    // `-Dmain=pic_main` renames the generated `main`, so it must apply to
    // prog.c only — host_main.c provides the real `main` and is compiled
    // without the rename, then the two objects are linked.
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-Dmain=pic_main", "-c"])
            .arg(c_path)
            .arg("-o")
            .arg(&obj_prog),
        "host clang (prog.c)",
    )?;
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-c"])
            .arg(&hm_path)
            .arg("-o")
            .arg(&obj_host),
        "host clang (host_main.c)",
    )?;
    run_ok(
        Command::new(&clang)
            .args(["-O1"])
            .arg(&obj_prog)
            .arg(&obj_host)
            .arg("-o")
            .arg(&exe),
        "host clang (link)",
    )?;

    let out = Command::new(&exe)
        .output()
        .map_err(|e| format!("run the host binary: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "host binary failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().ok_or("host binary printed nothing")?;
    line.trim()
        .parse::<u32>()
        .map_err(|_| format!("host binary printed a non-checksum line: {stdout:?}"))
}

/// The generated `host_main.c`: seeds each input global by name (matching the
/// sim-side RAM seeding byte-for-byte), calls the renamed `pic_main`, and
/// prints the checksum as a bare unsigned decimal.
fn host_main_source(program: &Program) -> Result<String, String> {
    // host_main.c is a separate translation unit: it needs the typedef
    // prologue itself so `extern volatile u32 in0;` matches the generated
    // globals' typedef'd types.
    let mut s = String::from("#include <stdio.h>\n");
    s.push_str(TYPEDEF_PROLOGUE);
    for input in &program.inputs {
        s.push_str(&format!(
            "extern volatile {} {};\n",
            width_type(input.width)?,
            input.name
        ));
    }
    s.push_str(&format!(
        "extern volatile unsigned char {};\n",
        program.checksum_name
    ));
    s.push_str("void pic_main(void);\nint main(void) {\n");
    for input in &program.inputs {
        s.push_str(&format!(
            "  {} = 0x{:X}u;\n",
            input.name,
            input.value & width_mask(input.width)
        ));
    }
    s.push_str(&format!(
        "  pic_main();\n  printf(\"%u\\n\", (unsigned){});\n  return 0;\n}}\n",
        program.checksum_name
    ));
    Ok(s)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// The PIC clang pair (`$PIC8_CLANG_UNWRAPPED` + `$PIC8_CLANG_RESOURCE_DIR`),
/// which the driver and the in-process layout pipeline both require.
fn pic_clang() -> Result<(String, String), String> {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED")
        .map_err(|_| "PIC8_CLANG_UNWRAPPED is not set (run inside `nix develop`)".to_string())?;
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").map_err(|_| {
        "PIC8_CLANG_RESOURCE_DIR is not set (run inside `nix develop`)".to_string()
    })?;
    Ok((clang, resdir))
}

/// The host clang: the nix shell's plain `clang` (the pinned clang WITHOUT
/// `-target`, whose wrapper knows the host toolchain — the unwrapped
/// `$PIC8_CLANG_UNWRAPPED` cannot find the host's stdio.h, verified during
/// development). `PIC8_HOST_CLANG` overrides it.
fn host_clang() -> String {
    std::env::var("PIC8_HOST_CLANG").unwrap_or_else(|_| "clang".to_string())
}

/// The volatile globals' addresses: run the same pipeline the driver runs
/// (mirroring `crates/driver/tests/long_e2e.rs`). Panics in the pipeline
/// (a compiler bug) are caught and reported as a failure, so the fuzz loop
/// survives them.
fn pic_layout(c_path: &Path) -> Result<alloc::AllocLayout, String> {
    let (clang, resdir) = pic_clang()?;
    let ll = Command::new(&clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
        ])
        .arg(c_path)
        .output()
        .map_err(|e| format!("run clang for the layout: {e}"))?;
    if !ll.status.success() {
        return Err(format!(
            "clang (layout) failed: {}",
            String::from_utf8_lossy(&ll.stderr)
        ));
    }
    let ll_text = String::from_utf8(ll.stdout).map_err(|e| format!("clang stdout: {e}"))?;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut m = irparse::parse_ll(&ll_text);
        m = wholeprog::merge(m);
        m = legalize::legalize(m);
        let cg = callgraph::build(&m);
        alloc::allocate(&m, &callgraph::edges_text(&cg))
    }))
    .map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        format!("compiler pipeline panic: {msg}")
    })
}

/// Run the driver binary (a workspace member) over the C file to produce the
/// hex, passing the PIC clang env vars it expects.
fn run_driver(c_path: &Path, hex_path: &Path) -> Result<(), String> {
    let (clang, resdir) = pic_clang()?;
    let driver = driver_binary()?;
    let out = Command::new(&driver)
        .arg(c_path)
        .arg(hex_path)
        .env("PIC8_CLANG_UNWRAPPED", &clang)
        .env("PIC8_CLANG_RESOURCE_DIR", &resdir)
        .output()
        .map_err(|e| format!("run the driver: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "driver failed (a compiler panic or an unsupported construct): {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Locate the driver binary, mirroring the driver crate's e2e pattern.
///
/// The e2e tests (inside `crates/driver`) use `env!("CARGO_BIN_EXE_driver")`,
/// which Cargo sets only for the package that owns the binary; this crate
/// instead finds the driver next to the running test executable in
/// `target/<profile>/` (the driver is a workspace member), honoring a
/// `PIC8_DRIVER` env override first.
///
/// The nested `cargo build -p driver` runs on EVERY first use (cheap when
/// up to date) — NOT only when the binary is missing: `cargo test -p fuzz`
/// does not rebuild the driver (fuzz does not depend on it), so a stale
/// binary from an earlier compiler build would otherwise silently run the
/// differential against outdated code (found when the corpus kept failing
/// with an already-fixed isel panic). The nested cargo cannot deadlock on
/// the build lock because tests run only after the outer build has finished
/// (verified empirically).
fn driver_binary() -> Result<PathBuf, String> {
    static CACHE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("PIC8_DRIVER") {
                return Ok(PathBuf::from(p));
            }
            if let Some(p) = option_env!("CARGO_BIN_EXE_driver") {
                return Ok(PathBuf::from(p));
            }
            let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            let mut dir = exe.clone();
            dir.pop(); // the exe name
            if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
                dir.pop(); // integration-test binaries live in target/<profile>/deps
            }
            let candidate = dir.join("driver");
            let mut cmd = Command::new("cargo");
            cmd.args(["build", "-p", "driver", "--quiet"]);
            if let Ok(profile) = std::env::var("PROFILE") {
                if profile != "debug" {
                    cmd.args(["--profile", &profile]);
                }
            }
            let status = cmd
                .status()
                .map_err(|e| format!("cargo build -p driver: {e}"))?;
            if !status.success() {
                return Err("cargo build -p driver failed".into());
            }
            if candidate.exists() {
                Ok(candidate)
            } else {
                Err(format!("driver binary not found at {}", candidate.display()))
            }
        })
        .clone()
}

fn run_ok(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

/// The C type name for a width under the typedef discipline (the host_main.c
/// externs must match the generated globals' typedef'd types exactly).
fn width_type(width: u8) -> Result<&'static str, String> {
    match width {
        8 => Ok("u8"),
        16 => Ok("u16"),
        32 => Ok("u32"),
        w => Err(format!("bad input width {w}")),
    }
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xFF,
        16 => 0xFFFF,
        32 => 0xFFFF_FFFF,
        w => panic!("bad input width {w}"),
    }
}

/// Seed `width` little-endian bytes of `value` at `addr` (the sim side of
/// the harness's identical seeding; the host side uses `host_main_source`).
fn seed_le(p: &mut pic14_sim::Pic14, addr: u16, width: u8, value: u32) {
    let bytes = match width {
        8 => 1,
        16 => 2,
        32 => 4,
        w => panic!("bad input width {w}"),
    };
    for i in 0..bytes {
        p.ram_mut()[addr as usize + i] = ((value >> (8 * i)) & 0xFF) as u8;
    }
}

fn read_le(ram: &[u8; 512], addr: u16, bytes: u8) -> u32 {
    let mut v = 0u32;
    for i in 0..bytes as usize {
        v |= (ram[addr as usize + i] as u32) << (8 * i);
    }
    v
}

/// A per-run scratch directory in the OS temp dir (unique per process + call
/// so parallel tests never collide).
struct WorkDir {
    path: PathBuf,
}

static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

impl WorkDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pic8-fuzz-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create the fuzz work dir");
        WorkDir { path }
    }
}
