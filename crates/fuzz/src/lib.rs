//! Seeded C generator + differential runner (PIC driver+sim vs host clang)
//! + greedy cvise-style reducer.
//!
//! Milestone 14's random-testing crate: the whole pipeline - a seeded
//! generator, a differential runner, a greedy reducer, and a corpus gate.
//! The generator emits a tiny, deterministic C program in the milestone's
//! "discipline" (unsigned-only arithmetic in genuinely explicit-width types
//! - `u8`/`u16`/`u32` from `TYPEDEF_PROLOGUE`, never bare `unsigned long`,
//! which is 64-bit on LP64 hosts - guarded shifts, a volatile `u8`
//! checksum); the differential runner compiles it twice  -
//! through the PIC8 driver into `pic14-sim`, and through host clang into a
//! native binary - seeds the volatile inputs identically on both sides, and
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
//!   the inputs by name) with the dev container's `clang` (the pinned clang
//!   WITHOUT `-target`; the unwrapped `$PIC8_CLANG_UNWRAPPED` cannot find the
//!   host's stdio.h) and reads the printed checksum.

use std::collections::HashMap;
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
/// as "32-bit on both msp430 and the host" - that equivalence is FALSE. On
/// LP64 hosts `unsigned long` is 64-bit, so a u32 computation whose result
/// exceeds 2^32 (e.g. `x * x` for x = 0xFFFFFFFF, or a 64-bit quotient)
/// diverges: msp430 wraps at 2^32, the host does not. `stdint.h` was the
/// first choice (`uint8_t`/`uint16_t`/`uint32_t` are exactly this), but it
/// does NOT resolve under the driver's fixed flags - `-nostdinc` drops the
/// builtin resource-dir include path (verified empirically; adding an
/// explicit `-isystem` fixes it, but the driver's flags are not ours to
/// change). So the robust option is self-contained typedefs guarded on the
/// target macro clang defines for the msp430 triple:
///
/// - u8  = unsigned char  (8 bits on both targets)
/// - u16 = unsigned short (16 bits on both targets)
/// - u32 = msp430: `unsigned long` (msp430 int is 16-bit, so its 32-bit
///        type is long) / host: `unsigned int` (32-bit on the pinned
///        x86-64-linux host) - genuinely 32-bit on BOTH sides.
///
/// Milestone 15 added the signed widths `s16`/`s32` for the float
/// conversions (`sitofp` needs a signed source, `fptosi` a signed target;
/// bare `int` is 16-bit on msp430 but 32-bit on the host): s16 = msp430
/// `int` / host `short`, s32 = msp430 `long` / host `int` - genuinely
/// 16/32-bit on both sides, same guard pattern.
///
/// Issue #14 added `s8` for the signed differential generator (signed
/// arithmetic/comparisons at 8 bits need an explicit s8 - `signed char` is
/// 8-bit on both targets).
///
/// With these, u8/u16/u32/s8/s16/s32 arithmetic wraps identically on both
/// sides and the differential is meaningful for values beyond 2^16 (pinned
/// by `u32_arithmetic_wraps_identically_on_both_sides` in tests/differential.rs
/// and by `unsigned_long_u32_arithmetic_mismatches`, which shows the old
/// discipline failing).
pub const TYPEDEF_PROLOGUE: &str = "\
#ifdef __MSP430__\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned long u32;\n\
typedef signed char s8;\n\
typedef int s16;\n\
typedef long s32;\n\
#else\n\
typedef unsigned char u8;\n\
typedef unsigned short u16;\n\
typedef unsigned int u32;\n\
typedef signed char s8;\n\
typedef short s16;\n\
typedef int s32;\n\
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

/// SplitMix64 - a small, self-contained, deterministic 64-bit PRNG.
///
/// Chosen over a bare LCG at the same zero-dependency cost: SplitMix64 is a
/// few lines, keeps only a 64-bit word of state, and - unlike an LCG, whose
/// low bits cycle visibly - mixes every output bit, so adjacent seeds produce
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

/// One volatile input global: `volatile unsigned <width> <name>;` (or
/// `volatile float <name>;` when `is_float`), seeded with `value` - for a
/// float input, the 4-byte IEEE-754 bit pattern - on both sides of the
/// differential (the sim seeds the RAM bytes; the host writes the bits
/// through a union, see `host_main_source`).
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub value: u32,
    pub width: u8, // 8 | 16 | 32 (a float input is width 32 + is_float)
    /// True for a `volatile float` input (its `value` is the bit pattern).
    pub is_float: bool,
}

/// A generated program: the C source plus the metadata the differential
/// harness needs to seed and observe it, and the generator's structural
/// knowledge (the main-body statements) the Task-3 reducer operates on.
#[derive(Debug, Clone)]
pub struct Program {
    pub c_source: String,
    pub inputs: Vec<Input>,
    pub checksum_name: String,
    /// The seed the program was generated from (provenance: the reduced
    /// fixture is named `reduced_<seed>.c`). Hand-written programs use a
    /// marker seed.
    pub seed: u64,
    /// The generator's structural knowledge: the main-body statements in
    /// source order. Scalar statements are single lines; block statements
    /// (if/else, loops) are one multi-line entry. The reducer's unit of
    /// deletion/rewrite.
    pub statements: Vec<String>,
    /// The source before the main body (the typedef prologue, the globals,
    /// the helper functions, `void main(void) {`). Invariant for generated
    /// programs: `c_source == prologue + statements.join("\n") + "\n}\n"`.
    pub prologue: String,
}

/// An IR-level differential program (issue #14): canonical IR text in the
/// `ir::parse` dialect (`global <name> <ty>` / `fn <name>(<ret>) (<params>)`
/// / `block <label>:` / `%d = <op> <ty> <a> <b>` - no LLVM `@`-global
/// definitions, no commas) fed DIRECTLY to the in-process pipeline  -
/// `ir::parse` -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
/// banking -> peephole -> asm - bypassing clang. The PIC side runs the
/// canonical IR; the host side runs the `c_twin` C source (the same
/// computation in the C discipline) so the differential still compares
/// checksums.
#[derive(Debug, Clone)]
pub struct IrProgram {
    /// Canonical IR text (`ir::parse` dialect).
    pub ir_text: String,
    /// The volatile input globals, seeded identically on both sides.
    pub inputs: Vec<Input>,
    /// The volatile checksum global's name.
    pub checksum_name: String,
    /// Provenance seed (the corpus contract).
    pub seed: u64,
    /// The C twin: the host-side oracle for the same computation.
    pub c_twin: String,
}

// ---------------------------------------------------------------------------
// Differential failures (Task 3: classified so the reducer can preserve
// the ORIGINAL failure)
// ---------------------------------------------------------------------------

/// The kind of a differential failure. The reducer accepts a candidate
/// deletion only when the failure it observed PERSISTS - the same kind - so
/// a candidate that merely breaks the build (e.g. a deletion that orphaned
/// a local) is rejected as a NEW failure, not the original one surviving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The PIC and host checksums disagree (a miscompile - the
    /// differential's core detection).
    Mismatch,
    /// The PIC compiler pipeline panicked or the driver failed (the
    /// loud-panic contract - a compiler bug).
    Panic,
    /// The simulator did not halt within the step budget.
    NoHalt,
    /// The program does not build/run on one side (the reducer's reject
    /// class: an invalid candidate is a NEW failure).
    Compile,
    /// The harness itself broke (IO, a missing global in the alloc map).
    Harness,
}

/// A differential failure: its kind (for the reducer's preservation check)
/// plus the human-readable message (the diagnostics the Task-1/2 tests
/// assert on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    message: String,
}

impl Failure {
    fn new(kind: FailureKind, message: String) -> Self {
        Failure { kind, message }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
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

/// The signed arith binops the signed generator can emit (mul needs the
/// width-space widening; add/sub/and/or/xor are plain unsigned-domain).
#[derive(Clone, Copy)]
enum SignedBin {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
}

/// The signed statement kinds (the forced rotation's pool).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SignedKind {
    Div,
    Rem,
    Ashr,
    Cmp,
    Mul,
    Add,
    Sub,
    Arith,
}

// ---------------------------------------------------------------------------
// Milestone 15: the float surface (float mode)
// ---------------------------------------------------------------------------

/// A float binary op the float generator can emit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FBin {
    Add,
    Sub,
    Mul,
    Div,
}

/// A float conversion the float generator can emit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FConvKind {
    UiToFp,
    SiToFp,
    FpToUi,
    FpToSi,
}

/// The float constant pool (float-mode operands). All values are
/// normal-range (biases 120..133) and nonzero except `0.0f`/`-0.0f`, which
/// are safe as fadd/fsub/fmul operands and fcmp comparands (0 op x is
/// exact) but excluded from the divisor pool. The constant pool is what
/// makes the -0.0 == +0.0 cmp case reachable (in6's single value cannot
/// be both signs).
const FCONSTS: &[&str] = &[
    "0.0f",
    "-0.0f",
    "1.0f",
    "-1.0f",
    "0.5f",
    "2.0f",
    "3.0f",
    "0.25f",
    "100.0f",
    "0.1f",
    "0.33333334f",
    "10.0f",
    "0.75f",
];

/// The divisor pool: FCONSTS minus the zeros.
const FCONSTS_NONZERO: &[&str] = &[
    "1.0f",
    "-1.0f",
    "0.5f",
    "2.0f",
    "3.0f",
    "0.25f",
    "100.0f",
    "0.1f",
    "0.33333334f",
    "10.0f",
    "0.75f",
];

/// The RNG-mix constants separating the float generator's streams from the
/// integer generator's (the corpus is deterministic either way; the mix
/// keeps adjacent int/float seeds visibly distinct).
const FLOAT_MIX: u64 = 0xF10A_7E5C_0000_0001;
const FLOAT_MIX2: u64 = 0xA5A5_1234_5678_9ABC;

/// A random NORMAL f32 bit pattern with the biased exponent in `lo..=hi`.
/// The band [100, 150] (values ~2^-27..2^23) is the safe arithmetic range
/// - the corpus's documented filter: NaN/inf/denormals are excluded, and
/// the operand pools keep every statement RESULT in the normal range too,
/// so the differential verifies RNE rounding without IEEE edge-case noise.
fn normal_bits(rng: &mut SplitMix64, lo: u32, hi: u32) -> u32 {
    let exp = lo + (rng.next_u64() as u32) % (hi - lo + 1);
    let mant = (rng.next_u64() as u32) & 0x7F_FFFF;
    let sign = ((rng.next_u64() as u32) & 1) << 31;
    sign | (exp << 23) | mant
}

/// The edge input in6: ±0, the smallest normals (0x00800000-ish - the
/// Task-3 cmp fix's boundary values), the RNE classics 1/3 and 0.1, or a
/// random normal with exponent 80..140 (still safe as an fadd/fsub
/// B-operand and as a cmp comparand).
fn edge_bits(rng: &mut SplitMix64) -> u32 {
    const EDGE: [u32; 8] = [
        0x0000_0000, // +0
        0x8000_0000, // -0
        0x0080_0000, // the smallest positive normal
        0x8080_0000, // the smallest negative normal
        0x3F80_0000, // 1.0
        0xBF80_0000, // -1.0
        0x3EAA_AAAB, // 1/3 (RNE)
        0x3DCC_CCCD, // 0.1 (RNE)
    ];
    match rng.next_u64() % 10 {
        0..=3 => EDGE[(rng.next_u64() as usize) % EDGE.len()],
        _ => normal_bits(rng, 80, 140),
    }
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
    /// live set (and therefore its frame, which the runtime routines'
    /// frames stack under; a straddling routine frame rounds into the next
    /// bank) small, the long_e2e budget is ≤ 9 i32 locals.
    locals: Vec<(String, u8)>,
    /// `(start, end)` index ranges of locals that died with their C block
    /// (if/else arms, loop bodies) - out of scope for later statements.
    dead: Vec<(usize, usize)>,
    /// The generated `main` body statements, one per line where possible
    /// (a flat, structurally-known shape for the Task-3 reducer).
    body: Vec<String>,
    used_fold16: bool,
    used_fold32: bool,
    /// Whether the array/struct globals are actually used (declared only
    /// then - every unused global byte costs main's frame budget).
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
    /// Milestone 15 float mode: the program is a float differential program
    /// (`generate_float`): float inputs in3..in6, float statements
    /// (fadd/fsub/fmul/fdiv/fcmp/conversions) folded through the volatile
    /// `fout` bits global, and its own globals-end (no array/struct, no int
    /// locals). Frame-budget estimates in this mode are exact def counts
    /// (measured from clang IR), so the fill margin is 0.
    float_mode: bool,
    /// Float locals (`float tN`) emitted so far - the float operand pool
    /// (a local's value is a normal-range float by construction, see
    /// `emit_fbin`; int locals never exist in float mode).
    flocals: Vec<String>,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Gen {
            rng: SplitMix64::new(seed),
            locals: Vec::new(),
            dead: Vec::new(),
            body: Vec::new(),
            used_fold16: false,
            used_fold32: false,
            used_array: false,
            used_struct: false,
            frame_est: 0,
            worst_routine: 0,
            forced: false,
            float_mode: false,
            flocals: Vec::new(),
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
    /// (always inside `width`'s range - constants never truncate).
    ///
    /// Only SAME-WIDTH inputs/locals are drawn: a cross-width cast would
    /// make clang materialize a zext/trunc def in main's frame, and the
    /// frame-budget model counts the statement's own defs, not the casts  -
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
        // in0 u8, in1 u16, in2 u32), found by position in INPUT_WIDTHS like
        // `operand` does - a w / 8 - 1 formula would yield the undeclared
        // in3 for w = 32.
        let i = INPUT_WIDTHS
            .iter()
            .position(|&x| x == w)
            .expect("operand_reg: no input of this width");
        let same_input = format!("({ct}){INPUT_PREFIX}{i}");
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
    /// frame end, and each must stay inside ONE GPR bank (issue #6, the
    /// recipe loops are skip-sensitive; alloc rounds a straddling routine
    /// frame into the next bank). Bank 0 holds 0x20..0x6F and the first
    /// routine frame ends no later than 0x6F (0x70-0x7F is common RAM,
    /// never used by locals); a larger routine frame simply spills into
    /// bank 1 wholesale instead of fitting bank 0, so the budget is the
    /// same 0x70 bound with the same measured routine frames (params +
    /// scratch):
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
        if self.float_mode {
            // Float-mode globals end (measured from the allocator, which
            // places even-aligned): in0 u8 @0x20, in3 float @0x22, in6 float
            // @0x26, checksum u8 @0x2A, fout float @0x2C - end 0x30. (No
            // array/struct in float mode.)
            return 0x70 - self.worst_routine - 0x30;
        }
        let globals =
            0x29 + if self.used_array { 8 } else { 0 } + if self.used_struct { 8 } else { 0 };
        0x70 - self.worst_routine - globals
    }

    /// Does a statement costing `frame` main bytes and requiring a
    /// `routine`-byte runtime frame still fit? (`uses_array`/`uses_struct`
    /// are the post-statement globals.)
    fn fit(&self, frame: u32, routine: u32, uses_array: bool, uses_struct: bool) -> bool {
        let globals = if self.float_mode {
            0x30 // the float-mode globals end (see `frame_budget`)
        } else {
            0x29 + if self.used_array || uses_array { 8 } else { 0 }
                + if self.used_struct || uses_struct {
                    8
                } else {
                    0
                }
        };
        let routine = self.worst_routine.max(routine);
        // The 8-byte safety margin applies to the FILL phase only: the
        // per-statement estimates are measured upper bounds (>= real), so
        // forced statements must fit by estimate alone - the margin would
        // reject real-fit flagged combos (e.g. array+struct: 35 est of a
        // 41 budget). The fill statements' cumulative real cost is still
        // bounded by est + 8 <= the hard bank-0 limit. Float mode's
        // estimates are exact def counts (measured from clang IR for the
        // fixed statement shapes), so no margin is needed there either.
        let margin = if self.forced || self.float_mode { 0 } else { 8 };
        self.frame_est + frame + margin <= 0x70 - routine - globals
    }

    /// The noinline byte-mix fold helpers (emitted only when used).
    fn fold_helpers_src(used16: bool, used32: bool) -> String {
        let mut s = String::new();
        // The trailing `+ (u8)in0` is a volatile read: without it the body
        // is pure arithmetic on the arg, so a constant-foldable arg makes
        // clang specialize the helper and dead-arg the original call into
        // `poison` (seed 2 - the IR pipeline cannot parse poison). in0 is
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

    /// Emit `{ct} tK = expr;` (declare-and-initialize - every generated
    /// local is single-assignment) + the fold, and return the local name.
    fn push_compute(&mut self, w: u8, expr: String) -> String {
        let t = self.new_local(w);
        self.body
            .push(format!("  {ct} {t} = {expr};", ct = ctype(w)));
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
        let mut w = if r < 50 {
            8
        } else if r < 80 {
            16
        } else {
            32
        };
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
        // forced op always runs at u8 - the cheapest width. The guarantee
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
        // pipeline cannot parse; u32 keeps `udiv i32` - see `operand_reg`).
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
                    format!(
                        "({ct})({l} {s} {c}u)",
                        ct = ctype(w),
                        s = if matches!(op, BinOp::Shl) { "<<" } else { ">>" }
                    )
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
        // A forced (flag-guaranteed) if always runs at u8 - the cheapest  -
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
        // The then-arm's locals die at the end of their block - mark them
        // dead BEFORE generating the else arm, so the else arm (and later
        // statements) can never reference them.
        let then = arm(self);
        self.dead.push((self.locals.len() - 1, self.locals.len()));
        let els = arm(self);
        self.body.push(format!(
            "  if ({cond}) {{\n{then}\n  }} else {{\n{els}\n  }}"
        ));
        self.dead.push((self.locals.len() - 1, self.locals.len()));
        true
    }

    /// A bounded loop: `for (i = 0; i < n; i++)` with n <= 8 (a masked
    /// input), body = 1–2 cheap inline ops on the accumulator (no runtime
    /// routines inside the trip loop), then fold the accumulator.
    fn emit_loop(&mut self) -> bool {
        // Bias the accumulator to u8 (a u32 loop's phi web costs ~4x a u8
        // one in main's frame and starves the mul/div statements of budget
        // - u32 math is covered by arith/struct/cmp instead). A forced
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
        // 1-2 body ops (each is a def in the phi web - keep the loop cheap).
        // NO `+`/`-` on the induction var: clang -O1 strength-reduces
        // `acc += i` over the masked bound into a closed-form sum in i9
        // magic arithmetic, which the IR pipeline cannot parse (found by
        // the corpus at seed 78). `^`/`&`/`|` are not sum idioms, so the
        // loop stays a real loop.
        let nops = 1 + self.below(2);
        for _ in 0..nops {
            let op = ["^", "&", "|"][self.below(3) as usize];
            body.push_str(&format!("    acc = ({ct})(({ct})acc {op} ({ct})i);\n"));
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
        // Operands FIRST - a local created before them would self-reference.
        let x = self.operand(16);
        let y = self.operand(8);
        let ix = self.new_local(8);
        self.body
            .push(format!("  u8 {ix} = (u8)((u16){x} % {n}u);"));
        self.body.push(format!("  arr[{ix}] = (u8){y};"));
        self.body
            .push(format!("  checksum = (u8)(checksum ^ (u8)arr[{ix}]);"));
        true
    }

    /// A struct statement: field-wise stores into the volatile global
    /// struct `s` (u8/u16/u32 fields), then a width-mixing fold over the
    /// fields (explicit casts; no layout dependence - names only).
    fn emit_struct(&mut self) -> bool {
        // Main-frame cost = the three operand locals + the field loads +
        // the s.c u32 shift/xor chain, MEASURED at 21-23 defs from clang
        // -O1 IR (the u32 fold is expensive in main's frame - the old
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
            "  checksum = (u8)(checksum ^ (u8)s.a ^ fold16((u16)s.b) ^ fold32((u32)s.c));"
                .to_string()
        } else {
            self.used_fold16 = true;
            "  checksum = (u8)(checksum ^ (u8)s.a ^ fold16((u16)s.b) ^ (u8)((u32)s.c >> 24u));"
                .to_string()
        };
        self.body.push(line);
        true
    }

    // ---- Issue #14: the signed surface (signed mode only) ----

    /// A signed width (s8/s16/s32 - the same byte widths as u8/u16/u32, so
    /// the frame-budget model and the local slots are shared).
    fn spick_width(&mut self) -> u8 {
        [8u8, 16, 32][self.below(3) as usize]
    }

    /// A signed arithmetic statement: `(sW)((uW)a op (uW)b)` - computed in
    /// the unsigned domain so wrapping is defined on BOTH sides (C's usual
    /// arithmetic conversions: on msp430 u16/u32 promote to the unsigned
    /// int/long of the same width and wrap mod 2^W; on the host the wider
    /// int holds the exact product and the cast truncates identically).
    /// `forced` pins the op (the flag-guaranteed first statement); the fill
    /// picks by the weighted mix. Main-frame costs mirror the unsigned
    /// arith table (same statement shapes); mul pulls the big routines.
    fn emit_sarith(&mut self, forced: Option<SignedBin>) -> bool {
        let mut w = self.spick_width();
        if self.forced {
            w = 8; // a flag-guaranteed statement runs at the cheapest width
        }
        let op = match forced {
            Some(op) => op,
            None => match self.below(6) {
                0 => SignedBin::Add,
                1 => SignedBin::Sub,
                2 => SignedBin::Mul,
                3 => SignedBin::And,
                4 => SignedBin::Or,
                _ => SignedBin::Xor,
            },
        };
        let (cost, routine) = match (w, op) {
            (_, SignedBin::Mul) => match w {
                8 => (8, 18),
                16 => (14, 22),
                _ => (16, 22),
            },
            (8, _) => (7, 0),
            (16, _) => (10, 0),
            _ => (15, 0),
        };
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        let ct = ctype(w); // the unsigned type of the same width
        let s = match op {
            SignedBin::Add => "+",
            SignedBin::Sub => "-",
            SignedBin::And => "&",
            SignedBin::Or => "|",
            _ => "^",
        };
        // Forced statements draw volatile/local operands only (a const
        // operand would let clang fold the op away - no coverage).
        let pick = |g: &mut Self, w: u8| -> String {
            if g.forced {
                g.operand_reg(w)
            } else {
                g.operand(w)
            }
        };
        let a = pick(self, w);
        let b = pick(self, w);
        // Mul widens to the next unsigned width so the host's promotion
        // cannot overflow (same discipline as the unsigned generator).
        let expr = match (op, w) {
            (SignedBin::Mul, 8) => format!("(s8)((u16)((u8){a}) * (u16)((u8){b}))"),
            (SignedBin::Mul, 16) => format!("(s16)((u32)((u16){a}) * (u32)((u16){b}))"),
            (SignedBin::Mul, _) => format!("(s32)(({a}) * ({b}))"),
            _ => format!("(s{w})(({ct}){a} {s} ({ct}){b})"),
        };
        let t = self.new_local(w);
        self.body.push(format!("  s{w} {t} = {expr};"));
        self.push_fold(w, &t);
        true
    }

    /// A signed div/rem statement with a CONSTANT divisor in 2..=9: the
    /// only signed-division UB pair is INT_MIN / -1, and the divisor is
    /// neither 0 nor -1, so no dividend guard is needed (signed const
    /// divisors stay plain `sdiv`/`srem` - clang does NOT magic-number
    /// strength-reduce signed division, verified; the unsigned generator's
    /// runtime-divisor rule is a udiv-only quirk). The dividend is a
    /// volatile input / local (never a constant - a const would fold).
    fn emit_sdivrem(&mut self, rem: bool) -> bool {
        let mut w = self.spick_width();
        if self.forced {
            w = 8; // a flag-guaranteed statement runs at the cheapest width
        }
        // Frames: __sdiv/__srem_i8 = 5, i16 = 7, i32 = 12 (the unsigned
        // table's 14/14/12 over-estimates - conservative, so reuse it).
        let (cost, routine) = match w {
            8 => (10, 14),
            16 => (12, 14),
            _ => (19, 12),
        };
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        let a = self.operand_reg(w); // volatile input / local dividend
        let k = 2 + self.below(8) as u32; // 2..=9: nonzero, never -1
        let op = if rem { "%" } else { "/" };
        let expr = format!("(s{w})((s{w}){a} {op} (s{w}){k})");
        let t = self.new_local(w);
        self.body.push(format!("  s{w} {t} = {expr};"));
        self.push_fold(w, &t);
        true
    }

    /// A signed arithmetic-shift statement: `(sW)((sW)v >> c)` - a const
    /// count in 1..=W-1, or a masked runtime count (volatile/local - a
    /// const count would fold the shift). ashr sign-fills, exercising the
    /// __ashr routines' sign extension.
    fn emit_sshift(&mut self) -> bool {
        let mut w = self.spick_width();
        if self.forced {
            w = 8; // a flag-guaranteed statement runs at the cheapest width
        }
        let (cost, routine) = match w {
            8 => (8, 3),
            16 => (10, 6),
            _ => (13, 12),
        };
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        let v = self.operand_reg(w);
        let expr = if self.below(3) < 2 {
            let c = 1 + self.below(u32::from(w) - 1);
            format!("(s{w})((s{w}){v} >> {c})")
        } else {
            let m = w - 1;
            let c = self.operand_reg(w);
            format!("(s{w})((s{w}){v} >> (s{w})((u{w}){c} & {m}u))")
        };
        let t = self.new_local(w);
        self.body.push(format!("  s{w} {t} = {expr};"));
        self.push_fold(w, &t);
        true
    }

    /// A signed comparison folded straight into the checksum:
    /// `checksum ^= (u8)((sW)a rel (sW)b)` - icmp slt/sle/sgt/sge/eq/ne.
    /// Volatile/local operands only (a const pair would fold the icmp).
    fn emit_scmp(&mut self) -> bool {
        let mut w = self.spick_width();
        if self.forced {
            w = 8; // a flag-guaranteed statement runs at the cheapest width
        }
        let cost = match w {
            8 => 7,
            16 => 9,
            _ => 13,
        };
        if !self.fit(cost, 0, false, false) {
            return false;
        }
        self.frame_est += cost;
        let rel = ["<", "<=", ">", ">=", "==", "!="][self.below(6) as usize];
        let a = self.operand_reg(w);
        let b = self.operand_reg(w);
        self.body.push(format!(
            "  checksum = (u8)(checksum ^ (u8)((s{w}){a} {rel} (s{w}){b}));"
        ));
        true
    }

    // ---- Milestone 15: the float surface (float mode) ----

    /// A float operand from the BAND pool: the band input in3 (the
    /// generated normal with the exponent in 100..150, value ~2^-27..2^23),
    /// a normal-range constant, or a recent float local. The band pool is
    /// the safe source for every arithmetic slot: values are normal and
    /// their arithmetic stays normal (the corpus's documented filter - no
    /// NaN/denormal/inf INPUTS, and the operand pools keep the RESULTS
    /// normal too, so the differential verifies RNE rounding without IEEE
    /// edge-case noise). Returns `(text, main-frame bytes the load costs)`.
    fn foperand_band(&mut self) -> (String, u32) {
        match self.below(4) {
            0 => ("in3".to_string(), 4),
            1 => (
                FCONSTS[self.below(FCONSTS.len() as u32) as usize].to_string(),
                0,
            ),
            _ => match self.recent_flocal() {
                Some(t) => (t, 0),
                None => ("in3".to_string(), 4),
            },
        }
    }

    /// A float operand from the ANY pool: the band pool plus the edge input
    /// in6 (±0, the smallest normals, and normals with exponents 80..140  -
    /// the Task-3 cmp fix's boundary values). The edge input is safe as a
    /// fcmp operand (comparisons are exact) and as an fadd/fsub B-operand
    /// (the band A-operand dominates, so A ± B stays in the normal range),
    /// but NOT for fmul/fdiv - the smallest normals would underflow to a
    /// denormal and zero would divide by zero.
    fn foperand_any(&mut self) -> (String, u32) {
        match self.below(5) {
            0 => ("in3".to_string(), 4),
            1 => ("in6".to_string(), 4),
            2 => (
                FCONSTS[self.below(FCONSTS.len() as u32) as usize].to_string(),
                0,
            ),
            _ => match self.recent_flocal() {
                Some(t) => (t, 0),
                None => ("in3".to_string(), 4),
            },
        }
    }

    /// A KNOWN-NONZERO divisor for fdiv: the band input in3 (exponent
    /// 100..150 - never zero) or a nonzero constant. Locals are excluded:
    /// their value is unknown to the generator, and a runtime zero divisor
    /// would diverge - the host computes IEEE ±inf (sign of the dividend)
    /// while the routine returns the deterministic +0x7F800000.
    fn fdivisor(&mut self) -> (String, u32) {
        match self.below(3) {
            0 => ("in3".to_string(), 4),
            _ => (
                FCONSTS_NONZERO[self.below(FCONSTS_NONZERO.len() as u32) as usize].to_string(),
                0,
            ),
        }
    }

    /// The 1st/2nd most recent float local (float mode has no blocks, so
    /// every float local stays live).
    fn recent_flocal(&mut self) -> Option<String> {
        if self.flocals.is_empty() {
            return None;
        }
        let want = 1 + self.below(2) as usize;
        let i = self.flocals.len().checked_sub(want)?;
        Some(self.flocals[i].clone())
    }

    fn new_flocal(&mut self) -> String {
        let name = format!("t{}", self.flocals.len());
        self.flocals.push(name.clone());
        name
    }

    /// The bits fold for a float RESULT: store it to the volatile `fout`
    /// global, re-read the four bytes as a u32 (the type-punned
    /// `*(volatile u32*)&fout` - LLVM opaque pointers make this a plain
    /// `load i32` of the float global's bytes, no bitcast inst), and fold
    /// through the shared fold32 helper. The fold is over the float's EXACT
    /// bits - a single wrong RNE bit changes the checksum.
    fn fpush_fold(&mut self, t: &str) {
        self.used_fold32 = true;
        self.body.push(format!("  fout = {t};"));
        self.body
            .push("  checksum = (u8)(checksum ^ fold32(*(volatile u32*)&fout));".to_string());
    }

    /// A float arithmetic statement: `float tN = a op b;` folded through the
    /// fout bits. The operand pools (see `foperand_band`/`foperand_any`/
    /// `fdivisor`) keep every RESULT in the normal range, so the routine
    /// only ever sees in-range RNE rounding. Main-frame cost = the operand
    /// loads + the fop def (4) + the bits fold (checksum load 1 + the i32
    /// bits load 4 + the fold32 call 1 + the xor 1 = 7), measured from
    /// clang IR.
    fn emit_fbin(&mut self, op: FBin, force_input: bool) -> bool {
        // The forced (first) statement must exercise the routine: with two
        // constant operands clang folds the op away (no call). Pin the
        // A-operand to the band input when forced.
        let (a, ac) = if force_input {
            ("in3".to_string(), 4)
        } else {
            self.foperand_band()
        };
        let (b, bc) = match op {
            FBin::Div => self.fdivisor(),
            FBin::Mul => self.foperand_band(),
            _ => self.foperand_any(), // add/sub: the edge input is safe as B
        };
        let cost = ac + bc + 4 + 7;
        // The routine's FULL frame (params + scratch) measured from the
        // alloc layout: __add_f32/__sub_f32/__mul_f32 = 4+4+14 = 22 bytes,
        // __div_f32 = 4+4+12 = 20. main_end + this must stay <= 0x70 (the
        // recipe slots are skip-sensitive; a straddling routine frame
        // rounds into bank 1 wholesale, so the budget keeps the frame in
        // bank 0), the M14 budget model's `worst_routine`.
        let routine = if matches!(op, FBin::Div) { 20 } else { 22 };
        if !self.fit(cost, routine, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(routine);
        let s = match op {
            FBin::Add => "+",
            FBin::Sub => "-",
            FBin::Mul => "*",
            FBin::Div => "/",
        };
        let t = self.new_flocal();
        self.body.push(format!("  float {t} = {a} {s} {b};"));
        self.fpush_fold(&t);
        true
    }

    /// A float comparison statement: `checksum = (u8)(checksum ^ (u8)(a rel
    /// b));` - the fcmp predicate materialized by legalize's __cmp_f32
    /// tri-state tree (the C ordered operators cover olt/ole/ogt/oge/oeq/
    /// one). Main-frame cost = the operand loads + the tree's worst shape
    /// (call 1 + 2 icmps 2 + select 1 + the zext 1 + the checksum load 1 +
    /// the xor 1 = 7), measured from clang IR.
    fn emit_fcmp_f(&mut self, force_input: bool) -> bool {
        // The forced (first) statement must exercise __cmp_f32: pin one
        // operand to an input (two constants would fold to a constant).
        let (a, ac) = if force_input {
            ("in3".to_string(), 4)
        } else {
            self.foperand_any()
        };
        let (b, bc) = self.foperand_any();
        let cost = ac + bc + 7;
        // __cmp_f32's full frame = params 8 + scratch 6 = 14 bytes.
        if !self.fit(cost, 14, false, false) {
            return false;
        }
        self.frame_est += cost;
        self.worst_routine = self.worst_routine.max(14);
        let rel = ["<", "<=", ">", ">=", "==", "!="][self.below(6) as usize];
        self.body.push(format!(
            "  checksum = (u8)(checksum ^ (u8)({a} {rel} {b}));"
        ));
        true
    }

    /// A float conversion statement. The sources are the bits of the band
    /// input in3 read through the type-punned load (any u32 - always
    /// defined), and the fptoui/fptosi targets are masked to ≤ 32767.5 so
    /// the conversion is ALWAYS in range (an out-of-range fptoui/fptosi is
    /// LLVM poison - the host could materialize anything, diverging from
    /// the routine's clamp). The `* 0.5f` makes odd masks fractional,
    /// exercising the truncation. Costs measured from clang IR.
    fn emit_fconv(&mut self, kind: FConvKind) -> bool {
        let src = "in3";
        match kind {
            FConvKind::UiToFp | FConvKind::SiToFp => {
                // load i32 (4) + the conv def (4) + the bits fold (7).
                let cost = 15;
                // The conversion routines' full frame = param 4 + scratch 8
                // = 12 bytes.
                if !self.fit(cost, 12, false, false) {
                    return false;
                }
                self.frame_est += cost;
                self.worst_routine = self.worst_routine.max(12);
                let expr = match kind {
                    FConvKind::UiToFp => format!("(float)(*(volatile u32*)&{src})"),
                    _ => format!("(float)(s32)(*(volatile u32*)&{src})"),
                };
                let t = self.new_flocal();
                self.body.push(format!("  float {t} = {expr};"));
                self.fpush_fold(&t);
            }
            FConvKind::FpToUi | FConvKind::FpToSi => {
                // load i32 (4) + and (4) + uitofp (4) + fmul (4) + the
                // conversion (4) + the fold (checksum 1 + call 1 + xor 1).
                let cost = 23;
                // The shape contains an fmul (`* 0.5f`): __mul_f32's FULL
                // frame (params 8 + scratch 14 = 22) dominates the
                // conversion routines' 12-byte frames - counting 12 let
                // seed 4's conv overflow bank 0 (the __mul_f32 slots at
                // 0xA0, found by the M15 float corpus).
                if !self.fit(cost, 22, false, false) {
                    return false;
                }
                self.frame_est += cost;
                self.worst_routine = self.worst_routine.max(22);
                // (float)(bits & 0xFFFF) * 0.5f <= 32767.5 - in range for
                // both the u32 and the s32 target (defined conversions).
                let line = match kind {
                    FConvKind::FpToUi => {
                        "  checksum = (u8)(checksum ^ fold32((u32)((float)((*(volatile u32*)&in3) & 0xFFFFu) * 0.5f)));"
                            .to_string()
                    }
                    _ => {
                        "  checksum = (u8)(checksum ^ fold32((u32)((s32)((float)(s32)((*(volatile u32*)&in3) & 0xFFFFu) * 0.5f))));"
                            .to_string()
                    }
                };
                self.body.push(line);
            }
        }
        true
    }

    /// Emit the helper functions (1–3): noinline, 0–3 unsigned params,
    /// u8 return. Bodies use only INLINE ops (add/sub/and/or/xor/const
    /// shifts/icmps - no mul/div/rem, whose runtime-routine frames would
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
            src.push_str(&format!("__attribute__((noinline)) u8 {name}({sig}) {{\n"));
            let mut prev: Vec<(String, u8)> = Vec::new();
            // One op PER PARAM first: every param must be referenced in the
            // body, or clang replaces the unused call arg with `poison`
            // (found by the corpus at seed 19 - the IR pipeline cannot
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
            // ops only - no mul/div/rem, whose runtime-routine frames
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
    // surface - pinned by the tests' coverage sanity checks). The heavy ops
    // (mul/div/rem pull in the big runtime routines) are flags too: forced
    // FIRST, while the frame budget is empty, so the random fill cannot
    // starve them.
    //
    // The flags are a BOUNDED random subset - exactly 2 of the 8, not
    // independent bits: main's frame (and the runtime routines' frames
    // stacked under it) is a hard hardware limit, so one program can
    // only hold a couple of heavy constructs. An unbounded bit-draw let a
    // seed's forced tail exceed the budget and was SILENTLY DROPPED
    // (review finding - 'guaranteed when flagged' was false); force() now
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
        inputs.push(Input {
            name,
            value,
            width: w,
            is_float: false,
        });
    }
    decls.push_str(&format!(
        "volatile u8 {checksum};\n",
        checksum = CHECKSUM_NAME
    ));

    let (helpers, helper_src) = g.emit_helpers();

    // Feature-flagged statements FIRST (frame budget empty, so the heavy
    // mul/div/rem and the structured constructs all fit), then a weighted
    // random fill bounded by the frame budget - the backend gives every
    // SSA def, volatile loads included, its own RAM slot, so main's frame,
    // and the runtime routines' frames stacked under it, cap the program's
    // size. While `forced` is set, the structured statements
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
            // trace, so fail loudly instead - the frame-budget model must
            // be recalibrated (cheaper forced variants, fewer simultaneous
            // flags) until every flagged construct fits.
            panic!(
                "fuzz: seed {seed}: flagged construct #{k} rejected by the frame budget \
                 (frame_est {}, worst_routine {}, budget {}) - the 'guaranteed when \
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
    let prologue = format!(
        "{TYPEDEF_PROLOGUE}{decls}{helper_src}{fold_src}void main(void) {{\n",
        fold_src = Gen::fold_helpers_src(g.used_fold16, g.used_fold32)
    );
    let statements = g.body;
    let c_source = format!("{prologue}{body}\n}}\n");
    Program {
        c_source,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
        seed,
        statements,
        prologue,
    }
}

// ---------------------------------------------------------------------------
// Milestone 15: the float differential (Task 5)
// ---------------------------------------------------------------------------

/// Generate a deterministic FLOAT differential program from `seed` - the
/// milestone's RNE verification at scale. The float inputs are random
/// IEEE-754 BIT PATTERNS under the documented corpus filter: NaN,
/// infinities, and denormals are EXCLUDED (the routines' IEEE edge-case
/// handling is deterministic-but-minimal and deferred - see the plan's
/// self-review notes); in3 is a normal with the exponent in the safe band
/// 100..150 (value ~2^-27..2^23, whose arithmetic stays in the normal
/// range), and in6 is the edge value (±0, the smallest normals
/// 0x00800000-ish, and normals with exponents 80..140 - covering the
/// Task-3 cmp fix: the sign-magnitude ordering, the zero equality, and the
/// smallest-normals boundary).
///
/// The statements cover the whole float surface - fadd/fsub/fmul/fdiv
/// (the four soft-float arithmetic routines), fcmp (the ordered C
/// predicates through legalize's __cmp_f32 tri-state tree), and the four
/// int↔float conversions (uitofp/sitofp/fptoui/fptosi) - with the operand
/// pools chosen so every RESULT also stays in the normal range (no
/// overflow/underflow/denormal noise; the differential then purely verifies
/// RNE rounding at scale). Every float result is folded over its BITS: the
/// volatile `fout` global is re-read as u32 (the type-punned load - a
/// single wrong RNE bit changes the fold), and the fold32 byte-mix feeds
/// the volatile u8 checksum. `in0` (u8) stays as the fold helper's
/// determinism anchor (same role as in the integer generator).
///
/// Every random choice comes from the seeded RNG in a fixed order, so
/// `seed` fully determines the program. The first (forced) statement's kind
/// rotates over the 6 families (add/sub/mul/div/cmp/conv), so across the
/// 50-seed corpus every float kind is guaranteed to appear (pinned by the
/// tests' coverage sanity check); the fill statements are best-effort
/// against the frame budget.
pub fn generate_float(seed: u64) -> Program {
    let mut g = Gen::new(seed ^ FLOAT_MIX);
    g.float_mode = true;
    g.used_fold32 = true; // every float program folds through fold32
    let mut rng = SplitMix64::new(seed ^ FLOAT_MIX2);

    // Inputs: in0 u8 (the fold anchor), in3 the band normal (exponent
    // 100..150 - the safe arithmetic source), in6 the edge value (±0, the
    // smallest normals, normals with exponents 80..140 - the cmp coverage).
    let mut inputs = Vec::new();
    inputs.push(Input {
        name: "in0".into(),
        value: (rng.next_u64() as u8) as u32,
        width: 8,
        is_float: false,
    });
    inputs.push(Input {
        name: "in3".into(),
        value: normal_bits(&mut rng, 100, 150),
        width: 32,
        is_float: true,
    });
    inputs.push(Input {
        name: "in6".into(),
        value: edge_bits(&mut rng),
        width: 32,
        is_float: true,
    });

    let mut decls = String::new();
    decls.push_str("volatile u8 in0;\n");
    for n in ["in3", "in6"] {
        decls.push_str(&format!("volatile float {n};\n"));
    }
    decls.push_str(&format!(
        "volatile u8 {checksum};\n",
        checksum = CHECKSUM_NAME
    ));
    decls.push_str("volatile float fout;\n");

    // The float statement families: 6 kinds (the fptoui/fptosi conversions
    // are part of Conv, drawn 4-way inside the dispatch).
    // The forced first statement rotates over the families (seed % 6), so
    // the corpus spans the surface by construction; the Conv sub-kind also
    // rotates ((seed / 6) % 4), so uitofp/sitofp/fptoui/fptosi each get
    // forced seeds in the corpus. Its operands come from the
    // inputs/constants (no locals yet) and its cost (<= 23) fits the empty
    // frame - a rejection means the budget model is broken.
    let forced: FloatKind = match seed % 6 {
        0 => FloatKind::Add,
        1 => FloatKind::Sub,
        2 => FloatKind::Mul,
        3 => FloatKind::Div,
        4 => FloatKind::Cmp,
        _ => FloatKind::Conv,
    };
    let forced_ok = if forced == FloatKind::Conv {
        let c = [
            FConvKind::UiToFp,
            FConvKind::SiToFp,
            FConvKind::FpToUi,
            FConvKind::FpToSi,
        ][(seed / 6) as usize % 4];
        g.emit_fconv(c)
    } else {
        emit_float_kind(&mut g, forced, true)
    };
    if !forced_ok {
        panic!(
            "fuzz: float seed {seed}: the forced statement was rejected by the frame budget \
             (frame_est {}, worst_routine {}, budget {}) - recalibrate the float budget model",
            g.frame_est,
            g.worst_routine,
            g.frame_budget()
        );
    }
    // Best-effort fill: weighted toward the arithmetic (the RNE heart of
    // the corpus), with cmp and the conversions as the supporting surface.
    for _ in 0..8 {
        let k = match g.below(100) {
            0..=9 => FloatKind::Add,
            10..=19 => FloatKind::Sub,
            20..=29 => FloatKind::Mul,
            30..=39 => FloatKind::Div,
            40..=55 => FloatKind::Cmp,
            _ => FloatKind::Conv,
        };
        if !emit_float_kind(&mut g, k, false) {
            break; // the frame budget is exhausted
        }
    }

    let body = g.body.join("\n");
    let prologue = format!(
        "{TYPEDEF_PROLOGUE}{decls}{fold_src}void main(void) {{\n",
        fold_src = Gen::fold_helpers_src(false, true)
    );
    let statements = g.body;
    let c_source = format!("{prologue}{body}\n}}\n");
    Program {
        c_source,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
        seed,
        statements,
        prologue,
    }
}

/// The float statement families the float generator can emit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FloatKind {
    Add,
    Sub,
    Mul,
    Div,
    Cmp,
    Conv,
}

/// Emit one float statement of `k`; false = the frame budget rejected it
/// (the fill loop stops; the forced statement panics instead - see
/// `generate_float`).
fn emit_float_kind(g: &mut Gen, k: FloatKind, force_input: bool) -> bool {
    match k {
        FloatKind::Add => g.emit_fbin(FBin::Add, force_input),
        FloatKind::Sub => g.emit_fbin(FBin::Sub, force_input),
        FloatKind::Mul => g.emit_fbin(FBin::Mul, force_input),
        FloatKind::Div => g.emit_fbin(FBin::Div, force_input),
        FloatKind::Cmp => g.emit_fcmp_f(force_input),
        FloatKind::Conv => {
            let c = [
                FConvKind::UiToFp,
                FConvKind::SiToFp,
                FConvKind::FpToUi,
                FConvKind::FpToSi,
            ][g.below(4) as usize];
            g.emit_fconv(c)
        }
    }
}

// ---------------------------------------------------------------------------
// Issue #14: the signed differential (wrap-safe signed arithmetic)
// ---------------------------------------------------------------------------

/// The RNG-mix constants separating the signed generator's streams from the
/// integer/float generators' (the corpus is deterministic either way; the
/// mix keeps adjacent int/float/signed seeds visibly distinct).
const SIGNED_MIX: u64 = 0x51ED_0000_0000_0001;
const SIGNED_MIX2: u64 = 0x51ED_0000_0000_0002;

/// Generate a deterministic SIGNED differential program from `seed` - issue
/// #14's signed surface. The inputs are the same fixed u8/u16/u32 volatile
/// globals as the integer generator (in0/in1/in2), read through `(sW)`
/// casts; the statements are the signed ops - sdiv/srem (const divisors
/// 2..=9, so the only signed-division UB pair INT_MIN / -1 is excluded by
/// construction), ashr (const or masked counts), the signed comparisons
/// (icmp slt/sle/sgt/sge/eq/ne folded straight into the checksum), and the
/// signed binops - all computed in the wrap-safe discipline:
///
/// - arithmetic computes in the UNSIGNED domain and re-casts
///   (`(sW)((uW)a op (uW)b)`), so wrapping is defined on BOTH sides (C's
///   usual arithmetic conversions: on msp430 u16/u32 promote to the
///   unsigned int/long of the same width and wrap mod 2^W; on the host the
///   wider int holds the exact result and the cast truncates identically);
/// - mul widens to the next unsigned width (u8 -> u16, u16 -> u32) so the
///   host's int promotion cannot overflow;
/// - div/rem use const divisors 2..=9 (never 0, never -1 - the only
///   host-UB pair is INT_MIN / -1, excluded by construction; signed const
///   divisors stay plain `sdiv`/`srem` - clang does NOT magic-number
///   strength-reduce signed division, verified);
/// - ashr results are folded width-preservingly (a signed shift truncated
///   to u8 would let clang prove the sign-fill irrelevant and lower it as
///   `lshr` - the fold reads the full width, so the ashr stays).
///
/// Every random choice comes from the seeded RNG in a fixed order, so
/// `seed` fully determines the program. The first (forced) statement's
/// kind rotates over the 8 signed families (seed % 8), so across the
/// corpus every signed kind is guaranteed to appear; the fill statements
/// are best-effort against the frame budget (the same bank-0 model as the
/// integer generator - the signed routines' frames are smaller than the
/// unsigned ones, so the budget is conservative).
pub fn generate_signed(seed: u64) -> Program {
    let mut g = Gen::new(seed ^ SIGNED_MIX);
    let mut rng = SplitMix64::new(seed ^ SIGNED_MIX2);

    // Inputs: the same fixed u8/u16/u32 mix as the integer generator (the
    // signed statements read them through `(sW)` casts; the wrap edges
    // 0x80/0x8000/0x80000000 are reachable when the RNG draws them).
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
        inputs.push(Input {
            name,
            value,
            width: w,
            is_float: false,
        });
    }
    decls.push_str(&format!(
        "volatile u8 {checksum};\n",
        checksum = CHECKSUM_NAME
    ));

    // The forced first statement rotates over the 8 signed families
    // (seed % 8), so the corpus spans the signed surface by construction.
    // Each forced statement runs at s8 (the cheapest width - the guarantee
    // is on the KIND, not the width) and fits the empty frame; a rejection
    // means the budget model is broken.
    let forced: SignedKind = match seed % 8 {
        0 => SignedKind::Div,
        1 => SignedKind::Rem,
        2 => SignedKind::Ashr,
        3 => SignedKind::Cmp,
        4 => SignedKind::Mul,
        5 => SignedKind::Add,
        6 => SignedKind::Sub,
        _ => SignedKind::Arith,
    };
    g.forced = true;
    let forced_ok = match forced {
        SignedKind::Div => g.emit_sdivrem(false),
        SignedKind::Rem => g.emit_sdivrem(true),
        SignedKind::Ashr => g.emit_sshift(),
        SignedKind::Cmp => g.emit_scmp(),
        SignedKind::Mul => g.emit_sarith(Some(SignedBin::Mul)),
        SignedKind::Add => g.emit_sarith(Some(SignedBin::Add)),
        SignedKind::Sub => g.emit_sarith(Some(SignedBin::Sub)),
        SignedKind::Arith => g.emit_sarith(None),
    };
    if !forced_ok {
        panic!(
            "fuzz: signed seed {seed}: the forced statement was rejected by the frame budget \
             (frame_est {}, worst_routine {}, budget {}) - recalibrate the signed budget model",
            g.frame_est,
            g.worst_routine,
            g.frame_budget()
        );
    }
    g.forced = false;

    // Best-effort fill: weighted toward div/rem/ashr/cmp (the signed
    // surface's heart), with the binops as the supporting surface.
    for _ in 0..10 {
        let k = match g.below(100) {
            0..=9 => SignedKind::Div,
            10..=19 => SignedKind::Rem,
            20..=34 => SignedKind::Ashr,
            35..=49 => SignedKind::Cmp,
            50..=59 => SignedKind::Mul,
            60..=74 => SignedKind::Arith,
            _ => {
                if g.below(2) == 0 {
                    SignedKind::Add
                } else {
                    SignedKind::Sub
                }
            }
        };
        let ok = match k {
            SignedKind::Div => g.emit_sdivrem(false),
            SignedKind::Rem => g.emit_sdivrem(true),
            SignedKind::Ashr => g.emit_sshift(),
            SignedKind::Cmp => g.emit_scmp(),
            SignedKind::Mul => g.emit_sarith(Some(SignedBin::Mul)),
            SignedKind::Add => g.emit_sarith(Some(SignedBin::Add)),
            SignedKind::Sub => g.emit_sarith(Some(SignedBin::Sub)),
            SignedKind::Arith => g.emit_sarith(None),
        };
        if !ok {
            break; // the frame budget is exhausted
        }
    }

    let body = g.body.join("\n");
    let prologue = format!(
        "{TYPEDEF_PROLOGUE}{decls}{fold_src}void main(void) {{\n",
        fold_src = Gen::fold_helpers_src(g.used_fold16, g.used_fold32)
    );
    let statements = g.body;
    let c_source = format!("{prologue}{body}\n}}\n");
    Program {
        c_source,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
        seed,
        statements,
        prologue,
    }
}

// ---------------------------------------------------------------------------
// Issue #14: the IR-level differential (canonical IR straight to the
// in-process pipeline - no clang, no driver binary)
// ---------------------------------------------------------------------------

/// The RNG-mix constants separating the IR generator's streams from the
/// integer/float/signed generators' (the corpus is deterministic either
/// way; the mix keeps adjacent seeds visibly distinct).
const IR_MIX: u64 = 0x1A5E_0000_0000_0001;
const IR_MIX2: u64 = 0x1A5E_0000_0000_0002;

/// Generate a deterministic IR-level differential program from `seed`  -
/// issue #14's IR mode. The PIC side runs the canonical IR text (the
/// `ir::parse` dialect: `global <name> <ty>` / `fn <name>(<ret>) (<params>)`
/// / `block <label>:` / `%d = <op> <ty> <a> <b>` - no LLVM `@`-global
/// definitions, no commas) DIRECTLY through the in-process pipeline  -
/// `ir::parse` -> wholeprog -> legalize -> callgraph -> alloc -> isel ->
/// banking -> peephole -> asm - bypassing clang and the driver binary. The
/// host side runs the `c_twin` C source (the same computation in the C
/// discipline) through host clang, so the differential still compares
/// checksums.
///
/// The statement pool covers the signed IR surface: `sdiv`/`srem` (const
/// divisors 2..=9 - the only signed-division UB pair INT_MIN / -1 is
/// excluded by construction), `ashr` (const counts), `icmp slt` (zext to
/// i8), plus `add`/`trunc` as the supporting surface and a rare i32 `sdiv`.
/// Every statement's result is folded into the volatile i8 `checksum`
/// global byte-wise (lo ^ hi for i16, lo ^ hi ^ next ^ top for i32), and
/// the C twin mirrors each statement and fold exactly.
///
/// Every random choice comes from the seeded RNG in a fixed order, so
/// `seed` fully determines the program (the corpus contract).
pub fn generate_ir(seed: u64) -> IrProgram {
    let mut rng = SplitMix64::new(seed ^ IR_MIX);
    let mut rng2 = SplitMix64::new(seed ^ IR_MIX2);

    // Inputs: in i16, in2 i32 (the signed surface's two widths; the wrap
    // edges 0x8000/0x80000000 are reachable when the RNG draws them).
    let in_val = rng.next_u64() as u16 as u32;
    let in2_val = rng.next_u64() as u32;
    let inputs = vec![
        Input {
            name: "in".into(),
            value: in_val,
            width: 16,
            is_float: false,
        },
        Input {
            name: "in2".into(),
            value: in2_val,
            width: 32,
            is_float: false,
        },
    ];

    let mut ir = String::new();
    let mut c = String::new();
    ir.push_str("global in i16\nglobal in2 i32\nglobal checksum i8\n");
    ir.push_str("fn main(void) ()\n  block entry:\n");
    c.push_str(TYPEDEF_PROLOGUE);
    c.push_str("volatile u16 in;\nvolatile u32 in2;\nvolatile u8 checksum;\nvoid main(void) {\n");

    // The two input loads (the only loads; every later operand is a
    // previous statement's result or a constant).
    let mut reg = 1u32;
    ir.push_str(&format!("%{reg} = load i16 @in\n"));
    reg += 1;
    ir.push_str(&format!("%{reg} = load i32 @in2\n"));
    reg += 1;
    let in_reg = "%1".to_string();
    let in2_reg = "%2".to_string();

    // 2..=4 statements (the frame budget: every SSA def gets its own RAM
    // slot, and the signed routines' frames must stay before the common-RAM
    // jump at 0x70 to fit bank 0; the fixed shapes are small enough that 4
    // statements fit comfortably; the i32 sdiv (the biggest routine, 20
    // bytes) is drawn at most once).
    let n = 2 + (rng2.next_u64() % 3) as usize;
    let mut last16: Option<String> = None; // IR reg of the last i16 result
    let mut last16_c: Option<String> = None; // its C local
    let mut used_i32 = false;
    let mut c_local = 0u32;

    for _ in 0..n {
        // Statement kind: 0 sdiv, 1 srem, 2 ashr, 3 add, 4 icmp slt,
        // 5 trunc, 6 i32 sdiv (rare).
        let kind = rng2.next_u64() % 7;
        let (ir_lines, c_line, res_reg, res_c, res_w): (Vec<String>, String, String, String, u8) =
            match kind {
                0 | 1 => {
                    // sdiv/srem i16 by a const 2..=9 (never 0, never -1).
                    let k = 2 + (rng2.next_u64() % 8);
                    let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                    let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                    let d = format!("%{reg}");
                    reg += 1;
                    let t = format!("t{c_local}");
                    c_local += 1;
                    let op = if kind == 0 { "sdiv" } else { "srem" };
                    let cop = if kind == 0 { "/" } else { "%" };
                    (
                        vec![format!("{d} = {op} i16 {a} {k}")],
                        format!("  s16 {t} = (s16)((s16){a_c} {cop} {k});"),
                        d,
                        t,
                        16u8,
                    )
                }
                2 => {
                    // ashr i16 by a const 1..=15 (sign-fill).
                    let k = 1 + (rng2.next_u64() % 15);
                    let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                    let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                    let d = format!("%{reg}");
                    reg += 1;
                    let t = format!("t{c_local}");
                    c_local += 1;
                    (
                        vec![format!("{d} = ashr i16 {a} {k}")],
                        format!("  s16 {t} = (s16)((s16){a_c} >> {k});"),
                        d,
                        t,
                        16u8,
                    )
                }
                3 => {
                    // add i16 (unsigned wrap - matches the C unsigned-domain
                    // discipline).
                    let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                    let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                    // Draw the b operand ONCE so the IR and the C twin pick
                    // the same source (a re-roll would diverge them).
                    let use_last = last16.is_some() && rng2.next_u64() % 2 == 0;
                    let (b, b_c) = if use_last {
                        (last16.clone().unwrap(), last16_c.clone().unwrap())
                    } else {
                        (in_reg.clone(), "in".to_string())
                    };
                    let d = format!("%{reg}");
                    reg += 1;
                    let t = format!("t{c_local}");
                    c_local += 1;
                    (
                        vec![format!("{d} = add i16 {a} {b}")],
                        format!("  s16 {t} = (s16)((u16){a_c} + (u16){b_c});"),
                        d,
                        t,
                        16u8,
                    )
                }
                4 => {
                    // icmp slt i16 vs 0, zext to i8.
                    let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                    let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                    let d = format!("%{reg}");
                    reg += 1;
                    let e = format!("%{reg}");
                    reg += 1;
                    let t = format!("t{c_local}");
                    c_local += 1;
                    (
                        vec![
                            format!("{d} = icmp slt i16 {a} 0"),
                            format!("{e} = zext i1 {d} to i8"),
                        ],
                        format!("  u8 {t} = (u8)((s16){a_c} < (s16)0);"),
                        e,
                        t,
                        8u8,
                    )
                }
                5 => {
                    // trunc i16 to i8 (the low byte).
                    let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                    let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                    let d = format!("%{reg}");
                    reg += 1;
                    let t = format!("t{c_local}");
                    c_local += 1;
                    (
                        vec![format!("{d} = trunc i16 {a} to i8")],
                        format!("  u8 {t} = (u8){a_c};"),
                        d,
                        t,
                        8u8,
                    )
                }
                _ => {
                    // i32 sdiv by a const 2..=9 (at most once - the biggest
                    // routine frame).
                    if used_i32 {
                        // Fall back to an i16 sdiv (the i32 slot is spent).
                        let k = 2 + (rng2.next_u64() % 8);
                        let a = last16.clone().unwrap_or_else(|| in_reg.clone());
                        let a_c = last16_c.clone().unwrap_or_else(|| "in".to_string());
                        let d = format!("%{reg}");
                        reg += 1;
                        let t = format!("t{c_local}");
                        c_local += 1;
                        (
                            vec![format!("{d} = sdiv i16 {a} {k}")],
                            format!("  s16 {t} = (s16)((s16){a_c} / {k});"),
                            d,
                            t,
                            16u8,
                        )
                    } else {
                        used_i32 = true;
                        let k = 2 + (rng2.next_u64() % 8);
                        let d = format!("%{reg}");
                        reg += 1;
                        let t = format!("t{c_local}");
                        c_local += 1;
                        (
                            vec![format!("{d} = sdiv i32 {in2_reg} {k}")],
                            format!("  s32 {t} = (s32)((s32)in2 / {k});"),
                            d,
                            t,
                            32u8,
                        )
                    }
                }
            };
        for line in &ir_lines {
            ir.push_str(line);
            ir.push('\n');
        }
        c.push_str(&c_line);
        c.push('\n');

        // Fold the result into the checksum (byte-wise, matching the C
        // twin's fold exactly).
        let (fold_ir, fold_c) = ir_fold_lines(&res_reg, &res_c, res_w, &mut reg);
        for line in &fold_ir {
            ir.push_str(line);
            ir.push('\n');
        }
        c.push_str(&fold_c);
        c.push('\n');

        match res_w {
            8 => {}
            _ => {
                last16 = Some(res_reg);
                last16_c = Some(res_c);
            }
        }
    }

    ir.push_str("    ret void\n");
    c.push_str("}\n");

    IrProgram {
        ir_text: ir,
        inputs,
        checksum_name: CHECKSUM_NAME.to_string(),
        seed,
        c_twin: c,
    }
}

/// The checksum fold for an IR statement result: xor the result's bytes
/// into the volatile i8 `checksum` global. Returns (IR lines, C twin
/// line). The C twin's `(u8)(t >> 8u)` etc. match the IR's `lshr`+`trunc`
/// byte extraction: the low byte of an arithmetic shift equals the low
/// byte of the logical shift (the sign-fill bits land in the dropped high
/// byte).
fn ir_fold_lines(ir_reg: &str, c_local: &str, width: u8, reg: &mut u32) -> (Vec<String>, String) {
    let mut ir_lines = Vec::new();
    let c = match width {
        8 => {
            let c_reg = format!("%{reg}");
            *reg += 1;
            let x = format!("%{reg}");
            *reg += 1;
            ir_lines.push(format!("{c_reg} = load i8 @checksum"));
            ir_lines.push(format!("{x} = xor i8 {c_reg} {ir_reg}"));
            ir_lines.push(format!("store i8 {x} @checksum"));
            format!("  checksum = (u8)(checksum ^ (u8){c_local});")
        }
        16 => {
            let lo = format!("%{reg}");
            *reg += 1;
            let hi = format!("%{reg}");
            *reg += 1;
            let hi8 = format!("%{reg}");
            *reg += 1;
            let c_reg = format!("%{reg}");
            *reg += 1;
            let x = format!("%{reg}");
            *reg += 1;
            let y = format!("%{reg}");
            *reg += 1;
            ir_lines.push(format!("{lo} = trunc i16 {ir_reg} to i8"));
            ir_lines.push(format!("{hi} = lshr i16 {ir_reg} 8"));
            ir_lines.push(format!("{hi8} = trunc i16 {hi} to i8"));
            ir_lines.push(format!("{c_reg} = load i8 @checksum"));
            ir_lines.push(format!("{x} = xor i8 {c_reg} {lo}"));
            ir_lines.push(format!("{y} = xor i8 {x} {hi8}"));
            ir_lines.push(format!("store i8 {y} @checksum"));
            format!("  checksum = (u8)(checksum ^ (u8){c_local} ^ (u8)({c_local} >> 8u));")
        }
        _ => {
            // i32: four bytes.
            let mut bytes = Vec::new();
            for shift in [0u32, 8, 16, 24] {
                if shift == 0 {
                    let b = format!("%{reg}");
                    *reg += 1;
                    ir_lines.push(format!("{b} = trunc i32 {ir_reg} to i8"));
                    bytes.push(b);
                } else {
                    let s = format!("%{reg}");
                    *reg += 1;
                    let b = format!("%{reg}");
                    *reg += 1;
                    ir_lines.push(format!("{s} = lshr i32 {ir_reg} {shift}"));
                    ir_lines.push(format!("{b} = trunc i32 {s} to i8"));
                    bytes.push(b);
                }
            }
            let c_reg = format!("%{reg}");
            *reg += 1;
            ir_lines.push(format!("{c_reg} = load i8 @checksum"));
            let mut acc = c_reg;
            for b in &bytes {
                let x = format!("%{reg}");
                *reg += 1;
                ir_lines.push(format!("{x} = xor i8 {acc} {b}"));
                acc = x;
            }
            ir_lines.push(format!("store i8 {acc} @checksum"));
            format!(
                "  checksum = (u8)(checksum ^ (u8){c_local} ^ (u8)({c_local} >> 8u) \
                 ^ (u8)({c_local} >> 16u) ^ (u8)({c_local} >> 24u));"
            )
        }
    };
    (ir_lines, c)
}

// ---------------------------------------------------------------------------
// Differential runner
// ---------------------------------------------------------------------------

/// Run the program on both sides and return the agreed checksum, or a
/// classified failure: a compile/driver error (including a compiler panic,
/// which surfaces as a failed process or a caught pipeline panic), a
/// non-halting sim run, or a host/PIC checksum mismatch. The classification
/// (`FailureKind`) is what the Task-3 reducer preserves.
pub fn run_differential(program: &Program, device: &device::Device) -> Result<u32, Failure> {
    let dir = WorkDir::new();
    let c_path = dir.path.join("prog.c");
    std::fs::write(&c_path, &program.c_source)
        .map_err(|e| Failure::new(FailureKind::Harness, format!("write prog.c: {e}")))?;

    let pic = run_pic(program, &c_path, &dir, device)?;
    let host = run_host(program, &c_path, &dir)?;

    if pic == host {
        Ok(pic)
    } else {
        Err(Failure::new(
            FailureKind::Mismatch,
            format!("mismatch: pic checksum {pic}, host checksum {host}"),
        ))
    }
}

/// Run an IR-level program on both sides and return the agreed checksum
/// (issue #14's IR mode): the PIC side runs the canonical IR through the
/// in-process pipeline - `ir::parse` -> wholeprog -> legalize -> callgraph
/// -> alloc -> isel -> banking -> peephole -> asm - bypassing clang and
/// the driver binary; the host side compiles the C twin with host clang
/// (the same computation in the C discipline). A pipeline panic is a
/// `Panic` failure (the loud-panic contract); a checksum disagreement a
/// `Mismatch`.
pub fn run_ir_differential(prog: &IrProgram, device: &device::Device) -> Result<u32, Failure> {
    let dir = WorkDir::new();
    let pic = run_ir_pic(prog, device)?;

    let twin_path = dir.path.join("twin.c");
    std::fs::write(&twin_path, &prog.c_twin)
        .map_err(|e| Failure::new(FailureKind::Harness, format!("write twin.c: {e}")))?;
    let host_prog = Program {
        c_source: prog.c_twin.clone(),
        inputs: prog.inputs.clone(),
        checksum_name: prog.checksum_name.clone(),
        seed: prog.seed,
        statements: Vec::new(),
        prologue: prog.c_twin.clone(),
    };
    let host = run_host(&host_prog, &twin_path, &dir)?;

    if pic == host {
        Ok(pic)
    } else {
        Err(Failure::new(
            FailureKind::Mismatch,
            format!("mismatch: pic checksum {pic}, host checksum {host}"),
        ))
    }
}

/// PIC side of the IR mode: the canonical IR through the in-process
/// pipeline (mirroring the driver's stage chain, minus clang), the hex
/// assembled in-process, `pic14-sim` seeded at the alloc addresses, run,
/// checksum read, `halted()` required. A pipeline panic (a compiler bug)
/// is caught and reported as a `Panic` failure, so the fuzz loop survives
/// them.
fn run_ir_pic(prog: &IrProgram, device: &device::Device) -> Result<u32, Failure> {
    let (hex, layout) = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut m = ir::parse(&prog.ir_text);
        m = wholeprog::merge(m);
        m = legalize::legalize(m);
        let cg = callgraph::build(&m);
        let layout = alloc::allocate(device, &m, &callgraph::edges_text(&cg));
        let mut addrs: HashMap<String, u16> = HashMap::new();
        addrs.extend(layout.globals.clone());
        addrs.extend(layout.locals.clone());
        let hex = match device.core {
            device::Core::Pic18 => {
                let asm = isel_pic18::select(device, &m, &addrs);
                asm::assemble_file_to_hex(device, &asm)
            }
            device::Core::Pic14 => {
                let asm = isel::select(device, &m, &addrs);
                let asm = banking::assign_banks(device, &asm);
                let asm = peephole::optimize(&asm);
                asm::assemble_file_to_hex(device, &asm)
            }
        };
        (hex, layout)
    }))
    .map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        Failure::new(
            FailureKind::Panic,
            format!("compiler pipeline panic: {msg}"),
        )
    })?;

    let checksum_addr = *layout.globals.get(&prog.checksum_name).ok_or_else(|| {
        Failure::new(
            FailureKind::Compile,
            format!("no global '{}' in the alloc map", prog.checksum_name),
        )
    })?;

    let checksum = match device.core {
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            for input in &prog.inputs {
                let addr = *layout.globals.get(&input.name).ok_or_else(|| {
                    Failure::new(
                        FailureKind::Compile,
                        format!("no global '{}' in the alloc map", input.name),
                    )
                })?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            p.run(MAX_SIM_STEPS);
            if !p.halted() {
                return Err(Failure::new(
                    FailureKind::NoHalt,
                    format!("simulator did not halt within {MAX_SIM_STEPS} steps"),
                ));
            }
            read_le(p.ram(), checksum_addr, 1) as u32
        }
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            for input in &prog.inputs {
                let addr = *layout.globals.get(&input.name).ok_or_else(|| {
                    Failure::new(
                        FailureKind::Compile,
                        format!("no global '{}' in the alloc map", input.name),
                    )
                })?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            p.run(MAX_SIM_STEPS);
            if !p.halted() {
                return Err(Failure::new(
                    FailureKind::NoHalt,
                    format!("simulator did not halt within {MAX_SIM_STEPS} steps"),
                ));
            }
            read_le(p.ram(), checksum_addr, 1) as u32
        }
    };
    Ok(checksum)
}

/// PIC side: alloc layout (in-process, mirroring the driver's e2e) for the
/// input/checksum addresses, the driver binary for the hex, `pic14-sim`
/// seeded at those addresses, run, checksum read, `halted()` required.
fn run_pic(
    program: &Program,
    c_path: &Path,
    dir: &WorkDir,
    device: &device::Device,
) -> Result<u32, Failure> {
    let layout = pic_layout(c_path, device)?;
    let checksum_addr = *layout.globals.get(&program.checksum_name).ok_or_else(|| {
        Failure::new(
            FailureKind::Compile,
            format!("no global '{}' in the alloc map", program.checksum_name),
        )
    })?;

    let hex_path = dir.path.join("prog.hex");
    run_driver(c_path, &hex_path, device)?;

    let hex = std::fs::read_to_string(&hex_path).map_err(|e| {
        Failure::new(
            FailureKind::Harness,
            format!("read {}: {e}", hex_path.display()),
        )
    })?;
    let checksum = match device.core {
        device::Core::Pic18 => {
            let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
            for input in &program.inputs {
                let addr = *layout.globals.get(&input.name).ok_or_else(|| {
                    Failure::new(
                        FailureKind::Compile,
                        format!("no global '{}' in the alloc map", input.name),
                    )
                })?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            p.run(MAX_SIM_STEPS);
            if !p.halted() {
                return Err(Failure::new(
                    FailureKind::NoHalt,
                    format!("simulator did not halt within {MAX_SIM_STEPS} steps"),
                ));
            }
            read_le(p.ram(), checksum_addr, 1) as u32
        }
        device::Core::Pic14 => {
            let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
            for input in &program.inputs {
                let addr = *layout.globals.get(&input.name).ok_or_else(|| {
                    Failure::new(
                        FailureKind::Compile,
                        format!("no global '{}' in the alloc map", input.name),
                    )
                })?;
                seed_le(p.ram_mut(), addr, input.width, input.value);
            }
            p.run(MAX_SIM_STEPS);
            if !p.halted() {
                return Err(Failure::new(
                    FailureKind::NoHalt,
                    format!("simulator did not halt within {MAX_SIM_STEPS} steps"),
                ));
            }
            read_le(p.ram(), checksum_addr, 1) as u32
        }
    };
    Ok(checksum)
}

/// Host side: compile `prog.c` + a generated `host_main.c` with host clang
/// (no `-target`), run the native binary, parse the printed checksum.
fn run_host(program: &Program, c_path: &Path, dir: &WorkDir) -> Result<u32, Failure> {
    let hm_path = dir.path.join("host_main.c");
    std::fs::write(
        &hm_path,
        host_main_source(program).map_err(|e| Failure::new(FailureKind::Compile, e))?,
    )
    .map_err(|e| Failure::new(FailureKind::Harness, format!("write host_main.c: {e}")))?;

    let clang = host_clang();
    let obj_prog = dir.path.join("prog_pic.o");
    let obj_host = dir.path.join("host_main.o");
    let exe = dir.path.join("prog");

    // `-Dmain=pic_main` renames the generated `main`, so it must apply to
    // prog.c only - host_main.c provides the real `main` and is compiled
    // without the rename, then the two objects are linked.
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-Dmain=pic_main", "-c"])
            .arg(c_path)
            .arg("-o")
            .arg(&obj_prog),
        "host clang (prog.c)",
    )
    .map_err(|e| Failure::new(FailureKind::Compile, e))?;
    run_ok(
        Command::new(&clang)
            .args(["-O1", "-c"])
            .arg(&hm_path)
            .arg("-o")
            .arg(&obj_host),
        "host clang (host_main.c)",
    )
    .map_err(|e| Failure::new(FailureKind::Compile, e))?;
    run_ok(
        Command::new(&clang)
            .args(["-O1"])
            .arg(&obj_prog)
            .arg(&obj_host)
            .arg("-o")
            .arg(&exe),
        "host clang (link)",
    )
    .map_err(|e| Failure::new(FailureKind::Compile, e))?;

    let out = Command::new(&exe)
        .output()
        .map_err(|e| Failure::new(FailureKind::Harness, format!("run the host binary: {e}")))?;
    if !out.status.success() {
        return Err(Failure::new(
            FailureKind::Compile,
            format!(
                "host binary failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next().ok_or_else(|| {
        Failure::new(
            FailureKind::Compile,
            "host binary printed nothing".to_string(),
        )
    })?;
    line.trim().parse::<u32>().map_err(|_| {
        Failure::new(
            FailureKind::Compile,
            format!("host binary printed a non-checksum line: {stdout:?}"),
        )
    })
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
        if input.is_float {
            s.push_str(&format!("extern volatile float {};\n", input.name));
        } else {
            s.push_str(&format!(
                "extern volatile {} {};\n",
                width_type(input.width)?,
                input.name
            ));
        }
    }
    s.push_str(&format!(
        "extern volatile unsigned char {};\n",
        program.checksum_name
    ));
    s.push_str("void pic_main(void);\nint main(void) {\n");
    for input in &program.inputs {
        if input.is_float {
            // Seed the float's BITS through a union - an assignment would
            // ROUND the bit pattern to the nearest float (0x3F800000 as an
            // unsigned int is not 1.0f). The union is host-side only.
            s.push_str(&format!(
                "  {{ union {{ unsigned int u; float f; }} cv; cv.u = 0x{:X}u; {} = cv.f; }}\n",
                input.value & 0xFFFF_FFFF,
                input.name
            ));
        } else {
            s.push_str(&format!(
                "  {} = 0x{:X}u;\n",
                input.name,
                input.value & width_mask(input.width)
            ));
        }
    }
    s.push_str(&format!(
        "  pic_main();\n  printf(\"%u\\n\", (unsigned){});\n  return 0;\n}}\n",
        program.checksum_name
    ));
    Ok(s)
}

// ---------------------------------------------------------------------------
// Reducer (Task 3: the greedy cvise-style reduction)
// ---------------------------------------------------------------------------

/// The reduction budget: at most this many differential re-runs per
/// `reduce` call (the plan's cap; the greedy fixed point normally converges
/// far below it).
pub const REDUCTION_CAP: usize = 5000;

/// The outcome of a reduction.
#[derive(Debug, Clone)]
pub struct ReducedProgram {
    /// The reduced program (same inputs/seed/checksum; fewer statements).
    pub program: Program,
    /// Differential re-runs performed (the verification run included).
    pub re_runs: usize,
    /// Statements deleted by the reduction.
    pub statements_removed: usize,
    /// Statements remaining.
    pub statements_kept: usize,
    /// The failure the reduction preserved.
    pub failure: Failure,
}

/// Greedily reduce `program` while `failure` persists: iterate over the
/// main-body statements (the generator's structural knowledge), try
/// deleting each statement - or replacing its expression with a constant /
/// one of its operands - re-run the differential, and keep the deletion
/// only when the SAME failure kind survives. Stop at a fixed point (a full
/// pass with no accepted change) or when `REDUCTION_CAP` re-runs are
/// exhausted.
///
/// `program` is verified to still exhibit `failure` first; its ACTUAL kind
/// AND message are taken from that verification run (robust against a stale
/// caller argument - the caller's message must not leak into the reduced
/// failure) and a differential-clean program is an error (nothing to
/// reduce). The reduced program is NOT written here - `write_fixture`
/// persists it as the `reduced_<seed>.c` artifact.
pub fn reduce(program: &Program, failure: &Failure) -> Result<ReducedProgram, String> {
    let fresh = match run_differential(program, &device::PIC16F877A) {
        Err(f) => f,
        Ok(_) => {
            return Err(format!(
                "reduce: the program is differential-clean (no {failure} to preserve)"
            ))
        }
    };
    let target = fresh.kind;
    let original_len = program.statements.len();
    let mut statements = program.statements.clone();
    let mut re_runs = 1usize; // the verification run above

    'reduce: loop {
        let mut i = 0usize;
        while i < statements.len() {
            for cand in candidates(&statements[i]) {
                if re_runs >= REDUCTION_CAP {
                    break 'reduce;
                }
                re_runs += 1;
                let candidate = match cand {
                    Some(text) => {
                        let mut stmts = statements.clone();
                        stmts[i] = text;
                        stmts
                    }
                    None => {
                        let mut stmts = statements.clone();
                        stmts.remove(i);
                        stmts
                    }
                };
                let probe = Program {
                    c_source: rebuild_source(&program.prologue, &candidate),
                    inputs: program.inputs.clone(),
                    checksum_name: program.checksum_name.clone(),
                    seed: program.seed,
                    statements: candidate,
                    prologue: program.prologue.clone(),
                };
                match run_differential(&probe, &device::PIC16F877A) {
                    Err(f) if f.kind == target => {
                        statements = probe.statements;
                        continue 'reduce; // restart the pass from the top
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        break; // a full pass with no accepted change: fixed point
    }

    Ok(ReducedProgram {
        program: Program {
            c_source: rebuild_source(&program.prologue, &statements),
            inputs: program.inputs.clone(),
            checksum_name: program.checksum_name.clone(),
            seed: program.seed,
            statements: statements.clone(),
            prologue: program.prologue.clone(),
        },
        re_runs,
        statements_removed: original_len.saturating_sub(statements.len()),
        statements_kept: statements.len(),
        failure: Failure {
            kind: target,
            message: fresh.message.clone(),
        },
    })
}

/// The reduction candidates for one statement, in preference order: `None`
/// = delete it; `Some(text)` = replace it with `text`. Deletion is always
/// tried first; expression replacement (with the constant `0u` or one of
/// the expression's top-level operands) applies to single-line assignments,
/// and ONLY when the replacement is strictly shorter - the well-founded
/// measure that makes the greedy terminate (without it, two equally-valid
/// short forms keep replacing each other and the pass never reaches the
/// fixed point).
fn candidates(stmt: &str) -> Vec<Option<String>> {
    let mut out = vec![None];
    if let Some((lhs, rhs)) = split_assignment(stmt) {
        let constant = format!("{lhs}0u;");
        if constant.len() < stmt.len() {
            out.push(Some(constant));
        }
        for op in top_level_operands(&rhs) {
            let repl = format!("{lhs}{op};");
            if repl.len() < stmt.len() {
                out.push(Some(repl));
            }
        }
    }
    out
}

/// Split a single-line assignment statement `… = …;` into its LHS prefix
/// (up to and including the `= `) and RHS (before the trailing `;`). Block
/// statements (if/else, loops) and non-assignment lines return None.
fn split_assignment(stmt: &str) -> Option<(String, String)> {
    if stmt.contains('\n') {
        return None;
    }
    let s = stmt.trim_end();
    if !s.ends_with(';') {
        return None;
    }
    let body = &s[..s.len() - 1];
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'=' if depth == 0 => {
                // The relational forms (`==`, `!=`, `<=`, `>=`) contain
                // `=` too, but in these statements they live inside
                // parenthesized operands - a depth-0 `=` is the assignment.
                // Guard the two-char forms anyway.
                let prev = if i > 0 { bytes[i - 1] } else { 0 };
                if matches!(prev, b'=' | b'!' | b'<' | b'>') {
                    i += 1;
                    continue;
                }
                let lhs = format!("{} ", &body[..=i]);
                let rhs = body[i + 1..].trim();
                if rhs.is_empty() {
                    return None;
                }
                return Some((lhs, rhs.to_string()));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The top-level operands of an expression: strip a leading result-cast
/// `(uN)( … )` (the generator's `({ct})(…)` shape), then split at the FIRST
/// binary operator at paren depth 0. Returns [] when there is no such
/// operator (a bare operand/constant - only constant replacement applies).
fn top_level_operands(expr: &str) -> Vec<String> {
    let mut inner = expr.trim();
    let b = inner.as_bytes();
    let cast = b.len() > 5
        && b[0] == b'('
        && b[1] == b'u'
        && matches!(b[2], b'8' | b'1' | b'3')
        && b[3] == b')'
        && b[4] == b'(';
    if cast {
        let mut depth = 0i32;
        let mut close = None;
        for (k, &c) in b.iter().enumerate().skip(4) {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(k) = close {
            if k == b.len() - 1 {
                inner = &inner[5..k]; // the whole expr is one cast: unwrap it
            }
        }
    }
    let bytes = inner.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0
                && matches!(
                    bytes[i],
                    b'+' | b'-'
                        | b'*'
                        | b'/'
                        | b'%'
                        | b'&'
                        | b'|'
                        | b'^'
                        | b'<'
                        | b'>'
                        | b'='
                        | b'!'
                ) =>
            {
                let two = bytes.get(i + 1).copied();
                let op_len = match (bytes[i], two) {
                    (b'<', Some(b'<'))
                    | (b'>', Some(b'>'))
                    | (b'<', Some(b'='))
                    | (b'>', Some(b'='))
                    | (b'=', Some(b'='))
                    | (b'!', Some(b'=')) => 2,
                    _ => 1,
                };
                let left = inner[..i].trim();
                let right = inner[i + op_len..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return vec![left.to_string(), right.to_string()];
                }
            }
            _ => {}
        }
        i += 1;
    }
    Vec::new()
}

/// Rebuild the full C source from the prologue + statements (the inverse of
/// `generate`'s assembly: `prologue + statements.join("\n") + "\n}\n"`).
fn rebuild_source(prologue: &str, statements: &[String]) -> String {
    format!("{prologue}{}\n}}\n", statements.join("\n"))
}

/// Save `program` as the `reduced_<seed>.c` fixture under `fixtures/`
/// (creating the directory), returning the saved path. The fixture is the
/// reduction artifact Task 4 commits for real bugs; synthetic reductions
/// (tests) clean it up after asserting.
pub fn write_fixture(program: &Program) -> Result<PathBuf, String> {
    let dir = Path::new("fixtures");
    std::fs::create_dir_all(dir).map_err(|e| format!("create fixtures/: {e}"))?;
    let path = dir.join(format!("reduced_{}.c", program.seed));
    std::fs::write(&path, &program.c_source)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// The PIC clang pair (`$PIC8_CLANG_UNWRAPPED` + `$PIC8_CLANG_RESOURCE_DIR`),
/// which the driver and the in-process layout pipeline both require.
fn pic_clang() -> Result<(String, String), String> {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").map_err(|_| {
        "PIC8_CLANG_UNWRAPPED is not set (run inside the dev container)".to_string()
    })?;
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").map_err(|_| {
        "PIC8_CLANG_RESOURCE_DIR is not set (run inside the dev container)".to_string()
    })?;
    Ok((clang, resdir))
}

/// The host clang: the dev container's plain `clang` (the pinned clang WITHOUT
/// `-target`, whose wrapper knows the host toolchain - the unwrapped
/// `$PIC8_CLANG_UNWRAPPED` cannot find the host's stdio.h, verified during
/// development). `PIC8_HOST_CLANG` overrides it.
fn host_clang() -> String {
    std::env::var("PIC8_HOST_CLANG").unwrap_or_else(|_| "clang".to_string())
}

/// The volatile globals' addresses: run the same pipeline the driver runs
/// (mirroring `crates/driver/tests/long_e2e.rs`). Panics in the pipeline
/// (a compiler bug) are caught and reported as a `Panic` failure, so the
/// fuzz loop survives them.
fn pic_layout(c_path: &Path, device: &device::Device) -> Result<alloc::AllocLayout, Failure> {
    let (clang, resdir) = pic_clang().map_err(|e| Failure::new(FailureKind::Harness, e))?;
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
        .map_err(|e| {
            Failure::new(
                FailureKind::Harness,
                format!("run clang for the layout: {e}"),
            )
        })?;
    if !ll.status.success() {
        return Err(Failure::new(
            FailureKind::Compile,
            format!(
                "clang (layout) failed: {}",
                String::from_utf8_lossy(&ll.stderr)
            ),
        ));
    }
    let ll_text = String::from_utf8(ll.stdout)
        .map_err(|e| Failure::new(FailureKind::Harness, format!("clang stdout: {e}")))?;
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut m = irparse::parse_ll(&ll_text);
        m = wholeprog::merge(m);
        m = legalize::legalize(m);
        let cg = callgraph::build(&m);
        alloc::allocate(device, &m, &callgraph::edges_text(&cg))
    }))
    .map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| p.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        Failure::new(
            FailureKind::Panic,
            format!("compiler pipeline panic: {msg}"),
        )
    })
}

/// Run the driver binary (a workspace member) over the C file to produce the
/// hex, passing the PIC clang env vars it expects. A failed driver is the
/// loud-panic contract: a compiler panic or an unsupported construct.
fn run_driver(c_path: &Path, hex_path: &Path, device: &device::Device) -> Result<(), Failure> {
    let (clang, resdir) = pic_clang().map_err(|e| Failure::new(FailureKind::Harness, e))?;
    let driver = driver_binary(device).map_err(|e| Failure::new(FailureKind::Harness, e))?;
    let out = Command::new(&driver)
        .arg(c_path)
        .arg("-o")
        .arg(hex_path)
        .args(["--device", device.name])
        .env("PIC8_CLANG_UNWRAPPED", &clang)
        .env("PIC8_CLANG_RESOURCE_DIR", &resdir)
        .output()
        .map_err(|e| Failure::new(FailureKind::Harness, format!("run the driver: {e}")))?;
    if !out.status.success() {
        return Err(Failure::new(
            FailureKind::Panic,
            format!(
                "driver failed (a compiler panic or an unsupported construct): {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        ));
    }
    Ok(())
}

/// Locate the driver binary, mirroring the driver crate's e2e pattern.
///
/// The e2e tests (inside `crates/driver`) use `env!("CARGO_BIN_EXE_epic-cc")`,
/// which Cargo sets only for the package that owns the binary; this crate
/// instead finds the driver next to the running test executable in
/// `target/<profile>/` (the driver is a workspace member), honoring a
/// `PIC8_DRIVER` env override first.
///
/// The nested `cargo build -p driver` runs on EVERY first use (cheap when
/// up to date) - NOT only when the binary is missing: `cargo test -p fuzz`
/// does not rebuild the driver (fuzz does not depend on it), so a stale
/// binary from an earlier compiler build would otherwise silently run the
/// differential against outdated code (found when the corpus kept failing
/// with an already-fixed isel panic). The nested cargo cannot deadlock on
/// the build lock because tests run only after the outer build has finished
/// (verified empirically).
fn driver_binary(device: &device::Device) -> Result<PathBuf, String> {
    fn locate() -> Result<PathBuf, String> {
        if let Some(p) = std::env::var_os("PIC8_DRIVER") {
            return Ok(PathBuf::from(p));
        }
        if let Some(p) = option_env!("CARGO_BIN_EXE_epic-cc") {
            return Ok(PathBuf::from(p));
        }
        let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        let mut dir = exe.clone();
        dir.pop();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("epic-cc");
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
            Err(format!(
                "driver binary not found at {}",
                candidate.display()
            ))
        }
    }
    static CACHE_P14: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    static CACHE_P18: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let cache = match device.core {
        device::Core::Pic18 => &CACHE_P18,
        device::Core::Pic14 => &CACHE_P14,
    };
    cache.get_or_init(locate).clone()
}

fn run_ok(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
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
fn seed_le(ram: &mut [u8], addr: u16, width: u8, value: u32) {
    let bytes = match width {
        8 => 1,
        16 => 2,
        32 => 4,
        w => panic!("bad input width {w}"),
    };
    for i in 0..bytes {
        ram[addr as usize + i] = ((value >> (8 * i)) & 0xFF) as u8;
    }
}

fn read_le(ram: &[u8], addr: u16, bytes: u8) -> u32 {
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
