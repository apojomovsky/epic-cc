//! Canonical IR text format shared by all pipeline stages. Text boundary:
//! every stage reads IR text in and writes IR text out.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    I1,
    I8,
    I16,
    I32,
    F32,
}
impl Ty {
    pub fn bytes(self) -> u8 {
        match self {
            Ty::I1 | Ty::I8 => 1,
            Ty::I16 => 2,
            Ty::I32 | Ty::F32 => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Reg(String),
    Const(i64),
    Global(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Mul,
    UDiv,
    URem,
    SDiv,
    SRem,
    Shl,
    LShr,
    AShr,
}

/// A GEP base: a named global (`@g`) or a pointer SSA register (`%r` — the
/// result of an alloca, a byval/sret param, or another GEP).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GepBase {
    Global(String),
    Reg(String),
}

#[derive(Clone, Debug)]
pub struct Load {
    pub dst: String,
    pub ty: Ty,
    pub ptr: String,
} // ptr = "@name" or "%name"
#[derive(Clone, Debug)]
pub struct Store {
    pub ty: Ty,
    pub val: Val,
    pub ptr: String,
}
#[derive(Clone, Debug)]
pub struct Bin {
    pub dst: String,
    pub op: BinOp,
    pub ty: Ty,
    pub a: Val,
    pub b: Val,
}
#[derive(Clone, Debug)]
pub struct Zext {
    pub dst: String,
    pub from: Ty,
    pub val: Val,
    pub to: Ty,
}
#[derive(Clone, Debug)]
/// `%d = inttoptr <from> <val> to ptr`: a runtime integer address becoming a
/// pointer VALUE. Kept distinct from `Zext` (which also parses to i16/i16)
/// because the dst slot of an `IntToPtr` holds a target ADDRESS, not an
/// ordinary value: `iselcore` seeds it as `Base::Slot(dst, true)` so every
/// load/store through it lowers as an indirect (FSR/INDF) access.
pub struct IntToPtr {
    pub dst: String,
    pub from: Ty,
    pub val: Val,
    pub to: Ty,
}
#[derive(Clone, Debug)]
pub struct Sext {
    pub dst: String,
    pub from: Ty,
    pub val: Val,
    pub to: Ty,
}
#[derive(Clone, Debug)]
pub struct Trunc {
    pub dst: String,
    pub from: Ty,
    pub val: Val,
    pub to: Ty,
}
#[derive(Clone, Debug)]
pub struct Icmp {
    pub dst: String,
    pub pred: String,
    pub ty: Ty,
    pub a: Val,
    pub b: Val,
}
#[derive(Clone, Debug)]
pub struct Select {
    pub dst: String,
    pub cond: Val,
    pub ty: Ty,
    pub a: Val,
    pub b: Val,
    /// True for a pointer-typed select (`select i1 c, ptr a, ptr b`): the
    /// result is a pointer VALUE, folded by iselcore like a GEP and emitted
    /// by neither backend (lowered at each load/store use). False for value
    /// selects (i1/i8/i16/f32), which both backends lower as a copy.
    pub ptr: bool,
}
/// A call argument. `ty` is `None` for pointer (`ptr`) args (byval/sret),
/// `Some` for scalar args. `byval`/`sret` are the phase-3 call ABI flags.
#[derive(Clone, Debug, PartialEq)]
pub struct CallArg {
    pub ty: Option<Ty>,
    pub val: Val,
    pub byval: Option<u8>,
    pub sret: bool,
}
#[derive(Clone, Debug)]
pub struct Call {
    pub dst: Option<String>,
    pub ty: Option<Ty>,
    pub func: String,
    pub args: Vec<CallArg>,
    /// Candidate targets of an indirect call (`func` is then the SSA
    /// register name, numeric). Empty for a direct call, whose target is
    /// `func`. Filled by `legalize` from the whole-program address-taken
    /// set; the canonical text round-trips it.
    pub callees: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Br {
    pub target: String,
}
#[derive(Clone, Debug)]
pub struct BrCond {
    pub cond: Val,
    pub t: String,
    pub f: String,
}
#[derive(Clone, Debug)]
pub struct Phi {
    pub dst: String,
    pub ty: Ty,
    pub incoming: Vec<(Val, String)>,
    /// True for a pointer-typed phi (`phi ptr [..]`): the result is a
    /// pointer VALUE. A pointer phi whose every incoming is a runtime
    /// address (a literal `Const` or a runtime-slot reg) is seeded by
    /// iselcore as an indirect slot; a phi with a compile-time (folded)
    /// arm keeps the loud unresolvable-chain panic.
    pub ptr: bool,
}
/// A getelementptr, reworked for structs/arrays: `base` is a global or a
/// pointer reg, `k` a constant byte offset, and `terms` scaled dynamic
/// offsets (`Σ scale×%reg`).
#[derive(Clone, Debug, PartialEq)]
pub struct Gep {
    pub dst: String,
    pub base: GepBase,
    pub k: u8,
    pub terms: Vec<(u8, String)>,
}
/// `alloca`: a local buffer of `size` bytes (virtual — isel allocates no
/// registers; alloc sizes the slot).
#[derive(Clone, Debug, PartialEq)]
pub struct Alloca {
    pub dst: String,
    pub size: u8,
}
/// `memcpy`: byte-copy `len` bytes from `src` to `dst` (defines nothing).
/// `len` is either a compile-time constant (unrolled per byte) or a 16-bit
/// register value (issue #4: runtime length — a counted loop; the value is
/// SSA-dead after the copy, so isel may decrement the length slot in place).
#[derive(Clone, Debug, PartialEq)]
pub enum MemLen {
    Const(u8),
    Reg(Val),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Memcpy {
    pub dst: Val,
    pub src: Val,
    pub len: MemLen,
}
/// `freeze`: LLVM freeze (`%d = freeze <ty> <val>`). A no-op in the backend —
/// it exists so the IR round-trips the source; isel lowers it as a plain byte
/// copy of `val` into the `dst` slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Freeze {
    pub dst: String,
    pub ty: Ty,
    pub val: Val,
}

/// The four float arithmetic ops (always f32 — msp430's float == f32).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FBinOp {
    FAdd,
    FSub,
    FMul,
    FDiv,
}
/// `%d = fadd float %a %b` — both operands and the dst are f32 (implicit).
#[derive(Clone, Debug, PartialEq)]
pub struct FloatBin {
    pub dst: String,
    pub op: FBinOp,
    pub a: Val,
    pub b: Val,
}
/// `%d = fcmp <pred> float %a %b` — the 16 LLVM float predicates; dst is i1.
#[derive(Clone, Debug, PartialEq)]
pub struct Fcmp {
    pub dst: String,
    pub pred: String,
    pub a: Val,
    pub b: Val,
}
/// The int<->float conversions and the f32->f32 casts (fpext/fptrunc are
/// no-ops on msp430 — double == float — but round-trip for the text).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatConvOp {
    FpToSi,
    FpToUi,
    SiToFp,
    UiToFp,
    Fpext,
    Fptrunc,
}
/// `%d = fptosi <from> <val> to <to>` (and the other five ops).
#[derive(Clone, Debug, PartialEq)]
pub struct FloatConv {
    pub dst: String,
    pub op: FloatConvOp,
    pub from: Ty,
    pub val: Val,
    pub to: Ty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AsmOperand {
    /// The LLVM constraint for this operand, e.g. `"*m"` or `"=*m"`.
    /// Only `*m` memory forms are valid on PIC; `r` is rejected earlier.
    pub constraint: String,
    /// The pointer value this operand names, canonical `ptr` form like
    /// `"@g"` or `"%x"` (the `%`/`@` prefix is included).
    pub ptr: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asm {
    pub template: String,
    pub clobbers_memory: bool,
    pub operands: Vec<AsmOperand>,
}

#[derive(Clone, Debug)]
pub enum Inst {
    Load(Load),
    Store(Store),
    Bin(Bin),
    Ret(Option<(Ty, Val)>),
    Zext(Zext),
    Sext(Sext),
    Trunc(Trunc),
    IntToPtr(IntToPtr),
    Icmp(Icmp),
    Select(Select),
    Call(Call),
    Br(Br),
    BrCond(BrCond),
    Phi(Phi),
    Gep(Gep),
    Alloca(Alloca),
    Memcpy(Memcpy),
    Freeze(Freeze),
    FloatBin(FloatBin),
    Fcmp(Fcmp),
    FloatConv(FloatConv),
    Asm(Asm),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub label: String,
    pub insts: Vec<Inst>,
}

/// A function parameter. `width` is the slot size in bytes: the scalar byte
/// width, the byval struct size (`byval`), or 2 for an sret pointer slot
/// (holds the target address). `ptr` marks a plain pointer param, whose slot
/// holds an address: width alone cannot say so, an `i16` is also 2 bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub width: u8,
    pub byval: Option<u8>,
    pub sret: bool,
    pub ptr: bool,
}
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub ret: Option<Ty>,
    pub params: Vec<Param>,
    pub blocks: Vec<Block>,
    /// True for an interrupt handler (`msp430_intrcc` in the .ll return
    /// position). Serialized as a `[isr]` marker between the ret group and
    /// the params group: `fn isr(void) [isr] ()`.
    pub isr: bool,
    pub naked: bool,
}

#[derive(Clone, Debug)]
pub struct Global {
    pub name: String,
    pub ty: Ty,
    pub is_const: bool,
    pub size: u16,
    pub bytes: Vec<u8>,
    pub addr: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub globals: Vec<Global>,
    pub funcs: Vec<Func>,
    pub module_asm: Vec<String>,
}

/// True for the legalize-injected runtime routines (the mul/div/rem/shift
/// and soft-float recipe bodies), including the interrupt-context `_isr`
/// copies. The recipe bodies are skip-sensitive (BTFSS/DECFSZ + GOTO,
/// INCFSZ + ADDWF), so a routine's frame must sit inside a single GPR bank:
/// `alloc` rounds routine bases and `isel` verifies the placement. Shared
/// by both stages; `legalize` injects exactly these names.
pub fn is_runtime_routine(name: &str) -> bool {
    let base = name.strip_suffix("_isr").unwrap_or(name);
    matches!(
        base,
        "__mul_u8"
            | "__mul_u16"
            | "__mul_u32"
            | "__udiv_u8"
            | "__urem_u8"
            | "__udiv_u16"
            | "__urem_u16"
            | "__udiv_u32"
            | "__urem_u32"
            | "__sdiv_i8"
            | "__srem_i8"
            | "__sdiv_i16"
            | "__srem_i16"
            | "__sdiv_i32"
            | "__srem_i32"
            | "__shl_u8"
            | "__lshr_u8"
            | "__ashr_i8"
            | "__shl_u16"
            | "__lshr_u16"
            | "__ashr_i16"
            | "__shl_u32"
            | "__lshr_u32"
            | "__ashr_i32"
            | "__add_f32"
            | "__sub_f32"
            | "__mul_f32"
            | "__div_f32"
            | "__cmp_f32"
            | "__uitofp_f32"
            | "__sitofp_f32"
            | "__fptoui_f32"
            | "__fptosi_f32"
    )
}

fn val_str(v: &Val) -> String {
    match v {
        Val::Reg(r) => format!("%{r}"),
        Val::Const(k) => k.to_string(),
        Val::Global(g) => format!("@{g}"),
    }
}

fn param_str(p: &Param) -> String {
    if let Some(n) = p.byval {
        format!("{}=byval{n}", p.name)
    } else if p.sret {
        format!("{}=sret", p.name)
    } else if p.ptr {
        format!("{}=ptr", p.name)
    } else {
        // scalar: encode the width so the text round-trips it (bare names
        // re-parse as width 1, silently undersizing i16 slots)
        match p.width {
            1 => format!("{}=i8", p.name),
            2 => format!("{}=i16", p.name),
            4 => format!("{}=i32", p.name),
            w => panic!(
                "ir: cannot serialize scalar param {} with width {w}",
                p.name
            ),
        }
    }
}

pub fn serialize(m: &Module) -> String {
    let mut out = String::new();
    for entry in &m.module_asm {
        out.push_str(&format!("module_asm \"{}\"\n", escape_asm(entry)));
    }
    for g in &m.globals {
        let kind = if g.is_const { "const" } else { "global" };
        match g.addr {
            Some(a) => out.push_str(&format!("{kind} {} {} @0x{a:02X}\n", g.name, ty_str(g.ty))),
            None => out.push_str(&format!("{kind} {} {}\n", g.name, ty_str(g.ty))),
        }
    }
    for f in &m.funcs {
        let ret = match f.ret {
            Some(t) => ty_str(t),
            None => "void".to_string(),
        };
        let params: Vec<String> = f.params.iter().map(param_str).collect();
        let mut markers = String::new();
        if f.isr {
            markers.push_str(" [isr]");
        }
        if f.naked {
            markers.push_str(" [naked]");
        }
        out.push_str(&format!(
            "fn {}({ret}){} ({})\n",
            f.name,
            markers,
            params.join(", ")
        ));
        for b in &f.blocks {
            out.push_str(&format!("  block {}:\n", b.label));
            for i in &b.insts {
                out.push_str(&format!("    {}\n", inst_str(i)));
            }
        }
    }
    out
}

fn escape_asm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape_asm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract the inner content of the first quoted string in `s` (starting at
/// the first `"`), handling `\"` and `\\` escapes, and return (unescaped
/// content, byte index after the closing `"`).
fn parse_quoted_unescaped(s: &str) -> (String, usize) {
    let bytes = s.as_bytes();
    let start = bytes
        .iter()
        .position(|&b| b == b'"')
        .expect("missing opening quote");
    let mut i = start + 1;
    let mut escaped = false;
    let mut end = None;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            end = Some(i);
            break;
        }
        i += 1;
    }
    let closing = end.expect("unterminated quoted string");
    let raw_inner = &s[start + 1..closing];
    (unescape_asm(raw_inner), closing + 1)
}

fn ty_str(t: Ty) -> String {
    match t {
        Ty::I1 => "i1".into(),
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::F32 => "float".into(),
    }
}

fn inst_str(i: &Inst) -> String {
    match i {
        Inst::Load(l) => format!("%{} = load {} {}", l.dst, ty_str(l.ty), l.ptr),
        Inst::Store(s) => format!("store {} {} {}", ty_str(s.ty), val_str(&s.val), s.ptr),
        Inst::Bin(b) => format!(
            "%{} = {} {} {} {}",
            b.dst,
            op_str(b.op),
            ty_str(b.ty),
            val_str(&b.a),
            val_str(&b.b)
        ),
        Inst::Ret(None) => "ret void".into(),
        Inst::Ret(Some((t, v))) => format!("ret {} {}", ty_str(*t), val_str(v)),
        Inst::Zext(z) => format!(
            "%{} = zext {} {} to {}",
            z.dst,
            ty_str(z.from),
            val_str(&z.val),
            ty_str(z.to)
        ),
        Inst::Sext(s) => format!(
            "%{} = sext {} {} to {}",
            s.dst,
            ty_str(s.from),
            val_str(&s.val),
            ty_str(s.to)
        ),
        Inst::Trunc(t) => format!(
            "%{} = trunc {} {} to {}",
            t.dst,
            ty_str(t.from),
            val_str(&t.val),
            ty_str(t.to)
        ),
        Inst::IntToPtr(p) => format!(
            "%{} = inttoptr {} {} to {}",
            p.dst,
            ty_str(p.from),
            val_str(&p.val),
            ty_str(p.to)
        ),
        Inst::Icmp(i) => format!(
            "%{} = icmp {} {} {} {}",
            i.dst,
            i.pred,
            ty_str(i.ty),
            val_str(&i.a),
            val_str(&i.b)
        ),
        Inst::Select(s) => {
            if s.ptr {
                format!(
                    "%{} = select i1 {} ptr {} ptr {}",
                    s.dst,
                    val_str(&s.cond),
                    val_str(&s.a),
                    val_str(&s.b)
                )
            } else {
                format!(
                    "%{} = select i1 {} {} {} {} {}",
                    s.dst,
                    val_str(&s.cond),
                    ty_str(s.ty),
                    val_str(&s.a),
                    ty_str(s.ty),
                    val_str(&s.b)
                )
            }
        }
        Inst::Call(c) => match (&c.ty, &c.dst) {
            (Some(t), Some(d)) => format!(
                "%{d} = call {} {}({}){}",
                ty_str(*t),
                callee_str(&c.func, &c.callees),
                call_args_str(&c.args),
                callees_str(&c.callees)
            ),
            _ => format!(
                "call void {}({}){}",
                callee_str(&c.func, &c.callees),
                call_args_str(&c.args),
                callees_str(&c.callees)
            ),
        },
        Inst::Br(b) => format!("br {}", b.target),
        Inst::BrCond(b) => format!("br i1 {} {} {}", val_str(&b.cond), b.t, b.f),
        Inst::Phi(p) => format!(
            "%{} = phi {} {}",
            p.dst,
            if p.ptr {
                "ptr".to_string()
            } else {
                ty_str(p.ty)
            },
            p.incoming
                .iter()
                .map(|(v, l)| format!("{} {}", val_str(v), l))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Inst::Gep(g) => {
            let base = match &g.base {
                GepBase::Global(n) => format!("@{n}"),
                GepBase::Reg(r) => format!("%{r}"),
            };
            let mut s = format!("%{} = gep {} +{}", g.dst, base, g.k);
            for (scale, reg) in &g.terms {
                s.push_str(&format!(" +{scale}*%{reg}"));
            }
            s
        }
        Inst::Alloca(a) => format!("%{} = alloca {}", a.dst, a.size),
        Inst::Memcpy(m) => format!(
            "memcpy {} {} {}",
            val_str(&m.dst),
            val_str(&m.src),
            match &m.len {
                MemLen::Const(n) => n.to_string(),
                MemLen::Reg(v) => val_str(v),
            }
        ),
        Inst::Freeze(f) => format!("%{} = freeze {} {}", f.dst, ty_str(f.ty), val_str(&f.val)),
        Inst::FloatBin(b) => format!(
            "%{} = {} float {} {}",
            b.dst,
            fbinop_str(b.op),
            val_str(&b.a),
            val_str(&b.b)
        ),
        Inst::Fcmp(c) => format!(
            "%{} = fcmp {} float {} {}",
            c.dst,
            c.pred,
            val_str(&c.a),
            val_str(&c.b)
        ),
        Inst::FloatConv(c) => format!(
            "%{} = {} {} {} to {}",
            c.dst,
            fconvop_str(c.op),
            ty_str(c.from),
            val_str(&c.val),
            ty_str(c.to)
        ),
        Inst::Asm(a) => {
            let mut s = format!("asm \"{}\"", escape_asm(&a.template));
            if a.clobbers_memory {
                s.push_str(" memory");
            }
            if !a.operands.is_empty() {
                s.push(' ');
                s.push_str(
                    &a.operands
                        .iter()
                        .map(|o| format!("{} {}", o.constraint, o.ptr))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            s
        }
    }
}
fn fbinop_str(o: FBinOp) -> &'static str {
    match o {
        FBinOp::FAdd => "fadd",
        FBinOp::FSub => "fsub",
        FBinOp::FMul => "fmul",
        FBinOp::FDiv => "fdiv",
    }
}

fn fconvop_str(o: FloatConvOp) -> &'static str {
    match o {
        FloatConvOp::FpToSi => "fptosi",
        FloatConvOp::FpToUi => "fptoui",
        FloatConvOp::SiToFp => "sitofp",
        FloatConvOp::UiToFp => "uitofp",
        FloatConvOp::Fpext => "fpext",
        FloatConvOp::Fptrunc => "fptrunc",
    }
}

fn call_arg_str(a: &CallArg) -> String {
    let mut s = String::new();
    if let Some(t) = a.ty {
        s.push_str(&ty_str(t));
        s.push(' ');
    }
    if let Some(n) = a.byval {
        s.push_str(&format!("byval{n} "));
    }
    if a.sret {
        s.push_str("sret ");
    }
    s.push_str(&val_str(&a.val));
    s
}

fn call_args_str(args: &[CallArg]) -> String {
    args.iter().map(call_arg_str).collect::<Vec<_>>().join(", ")
}

/// The callee token: `@name` for a direct call, `%reg` for an indirect one
/// (whose `func` is the SSA register name and `callees` is non-empty).
fn callee_str(func: &str, callees: &[String]) -> String {
    if callees.is_empty() {
        format!("@{func}")
    } else {
        format!("%{func}")
    }
}

/// The `callees <f0> <f1> ...` suffix for an indirect call's canonical text.
/// Empty for a direct call (no suffix).
fn callees_str(callees: &[String]) -> String {
    if callees.is_empty() {
        String::new()
    } else {
        format!(" callees {}", callees.join(" "))
    }
}

fn op_str(o: BinOp) -> &'static str {
    match o {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Xor => "xor",
        BinOp::Mul => "mul",
        BinOp::UDiv => "udiv",
        BinOp::URem => "urem",
        BinOp::SDiv => "sdiv",
        BinOp::SRem => "srem",
        BinOp::Shl => "shl",
        BinOp::LShr => "lshr",
        BinOp::AShr => "ashr",
    }
}

/// Index of the `)` matching the `(` at `open` in `s`.
fn matching_paren(s: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return open + i;
        }
    }
    panic!("unbalanced parens in {s:?}");
}

pub fn parse(text: &str) -> Module {
    let mut globals = Vec::new();
    let mut funcs = Vec::new();
    let mut module_asm = Vec::new();
    let mut cur_func: Option<Func> = None;
    let mut cur_block: Option<Block> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("module_asm ") {
            let (decoded, _) = parse_quoted_unescaped(line);
            module_asm.push(decoded);
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("global ")
            .or_else(|| line.strip_prefix("const "))
        {
            let is_const = line.starts_with("const ");
            let parts: Vec<&str> = rest.split_whitespace().collect();
            // format: <name> <ty> [@addr]
            let name = parts[0].to_string();
            let ty = parse_ty(parts[1]);
            let addr = if parts.len() >= 3 {
                parse_addr(parts[2])
            } else {
                None
            };
            globals.push(Global {
                name,
                ty,
                is_const,
                size: u16::from(ty.bytes()),
                bytes: Vec::new(),
                addr,
            });
        } else if line.starts_with("fn ") {
            let rest = &line[3..];
            let open = rest.find('(').unwrap();
            let name = rest[..open].trim().to_string();
            let ret_close = matching_paren(rest, open);
            let ret_str = rest[open + 1..ret_close].trim();
            let after = &rest[ret_close + 1..];
            let p_open = after
                .find('(')
                .expect("fn header must have a params group: fn <name>(<ret>) (<params>)");
            // the optional `[isr]` and `[naked]` markers live between the ret group and
            // the params group, in any order
            let before_params = &after[..p_open];
            let isr = before_params.contains("[isr]");
            let naked = before_params.contains("[naked]");
            let p_close = matching_paren(after, p_open);
            let p_str = &after[p_open + 1..p_close];
            let ret = if ret_str == "void" || ret_str.is_empty() {
                None
            } else {
                Some(parse_ty(ret_str))
            };
            let params = if p_str.trim().is_empty() {
                vec![]
            } else {
                p_str.split(',').map(parse_param).collect()
            };
            if let Some(f) = cur_func.as_mut() {
                if let Some(b) = cur_block.take() {
                    f.blocks.push(b);
                }
            }
            if let Some(f) = cur_func.take() {
                funcs.push(f);
            }
            cur_func = Some(Func {
                name,
                ret,
                params,
                blocks: Vec::new(),
                isr,
                naked,
            });
        } else if line == "}" || line == "{" {
            continue;
        } else if line.ends_with(':') {
            // Support both `block foo:` (canonical) and `foo:` (brace-style test
            // input from CC-4 Task 1). The brace style is e.g. `entry:` inside
            // `fn foo() [naked] () { entry: asm ... }`.
            if let Some(f) = cur_func.as_mut() {
                if let Some(b) = cur_block.take() {
                    f.blocks.push(b);
                }
            }
            let raw = line.trim_end_matches(':').trim();
            let label = if let Some(rest) = raw.strip_prefix("block ") {
                rest.trim().to_string()
            } else {
                raw.to_string()
            };
            if label.is_empty() {
                continue;
            }
            cur_block = Some(Block {
                label,
                insts: Vec::new(),
            });
        } else {
            let inst = parse_inst(line);
            match (&mut cur_func, &mut cur_block) {
                (Some(_), Some(b)) => b.insts.push(inst),
                (Some(_), None) => panic!("instruction before any block: {line}"),
                (None, _) => panic!("instruction outside a function: {line}"),
            }
        }
    }
    if let Some(f) = cur_func.as_mut() {
        if let Some(b) = cur_block.take() {
            f.blocks.push(b);
        }
    }
    if let Some(f) = cur_func.take() {
        funcs.push(f);
    }
    Module {
        globals,
        funcs,
        module_asm,
    }
}

fn parse_ty(s: &str) -> Ty {
    match s {
        "i1" => Ty::I1,
        "i8" => Ty::I8,
        "i16" => Ty::I16,
        "i32" => Ty::I32,
        "float" | "f32" => Ty::F32,
        other => panic!("unsupported type {other}"),
    }
}
fn parse_addr(s: &str) -> Option<u16> {
    s.strip_prefix('@')
        .map(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).unwrap())
}
fn parse_val(s: &str) -> Val {
    let s = s.trim_end_matches(',');
    if let Some(r) = s.strip_prefix('%') {
        Val::Reg(r.to_string())
    } else if let Some(g) = s.strip_prefix('@') {
        Val::Global(g.to_string())
    } else {
        Val::Const(s.parse().unwrap_or_else(|_| panic!("bad value {s}")))
    }
}

/// Parse one canonical param token: `<name>=i8` | `<name>=i16` |
/// `<name>=ptr` | `<name>=byval<N>` | `<name>=sret`. Bare `<name>` (width-1
/// shorthand, retained for backward compatibility) also parses.
fn parse_param(s: &str) -> Param {
    let s = s.trim();
    let (name, rest) = match s.find('=') {
        Some(i) => (s[..i].trim().to_string(), s[i + 1..].trim()),
        None => (s.to_string(), ""),
    };
    let name = name.trim_start_matches('%').to_string();
    if let Some(n) = rest.strip_prefix("byval") {
        let n = n.parse::<u8>().unwrap();
        Param {
            name,
            width: n,
            byval: Some(n),
            sret: false,
            ptr: false,
        }
    } else if rest == "sret" {
        Param {
            name,
            width: 2,
            byval: None,
            sret: true,
            ptr: false,
        }
    } else if rest == "ptr" {
        Param {
            name,
            width: 2,
            byval: None,
            sret: false,
            ptr: true,
        }
    } else if matches!(rest, "i1" | "i8" | "i16" | "i32" | "float" | "f32") {
        Param {
            name,
            width: parse_ty(rest).bytes(),
            byval: None,
            sret: false,
            ptr: false,
        }
    } else if rest.is_empty() {
        Param {
            name,
            width: 1,
            byval: None,
            sret: false,
            ptr: false,
        }
    } else {
        panic!("malformed param {s:?}")
    }
}

/// Parse one canonical call arg: `[i1|i8|i16] [byval<N>] [sret] <val>`.
fn parse_call_arg(s: &str) -> CallArg {
    let mut ty = None;
    let mut byval = None;
    let mut sret = false;
    let mut val_tok = None;
    for tok in s.trim().split_whitespace() {
        match tok {
            "i1" | "i8" | "i16" | "i32" | "float" | "f32" => ty = Some(parse_ty(tok)),
            _ => {
                if let Some(n) = tok.strip_prefix("byval") {
                    byval = Some(n.parse().unwrap());
                } else if tok == "sret" {
                    sret = true;
                } else {
                    val_tok = Some(tok);
                }
            }
        }
    }
    CallArg {
        ty,
        val: parse_val(val_tok.expect("call arg must carry a value")),
        byval,
        sret,
    }
}

fn parse_call(rest: &str) -> (Option<Ty>, String, Vec<CallArg>, Vec<String>) {
    // The callee is the first `@name` (direct) or `%reg` (indirect) token
    // after the return type. `find('@')` alone would miss an indirect call.
    let at = rest.find('@');
    let pct = rest.find('%');
    let callee_pos = match (at, pct) {
        (Some(a), Some(p)) => Some(a.min(p)),
        (Some(a), None) => Some(a),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    };
    let callee_pos = callee_pos.expect("call must have a callee");
    let ty_part = rest[..callee_pos].trim();
    let ty = if ty_part == "void" {
        None
    } else {
        Some(parse_ty(ty_part))
    };
    let open = rest.find('(').unwrap();
    let func = rest[callee_pos + 1..open].trim().to_string();
    let close = matching_paren(rest, open);
    let args = if open + 1 == close {
        vec![]
    } else {
        rest[open + 1..close]
            .split(',')
            .map(parse_call_arg)
            .collect()
    };
    // Optional `callees <f0> <f1> ...` suffix after the closing paren.
    let tail = rest[close + 1..].trim();
    let callees = if let Some(list) = tail.strip_prefix("callees") {
        list.split_whitespace().map(str::to_string).collect()
    } else {
        vec![]
    };
    (ty, func, args, callees)
}

fn parse_gep_expr(rest: &str) -> (GepBase, u8, Vec<(u8, String)>) {
    let mut it = rest.split_whitespace();
    let base_tok = it.next().unwrap();
    let base = if let Some(g) = base_tok.strip_prefix('@') {
        GepBase::Global(g.to_string())
    } else {
        GepBase::Reg(base_tok.trim_start_matches('%').to_string())
    };
    let k = it
        .next()
        .unwrap()
        .trim_start_matches('+')
        .parse::<u8>()
        .unwrap();
    let mut terms = Vec::new();
    for t in it {
        let t = t.trim_start_matches('+');
        let star = t.find('*').unwrap();
        let s = t[..star].parse::<u8>().unwrap();
        let r = t[star + 1..].trim_start_matches('%').to_string();
        terms.push((s, r));
    }
    (base, k, terms)
}

fn parse_inst(line: &str) -> Inst {
    // CC-4 asm: `asm "template" [memory] [[constraint ptr], ...]`
    // `memory` is the clobber marker; operands are `constraint ptr` pairs
    // like `*m @x` or `=*m %y` (only `*m` forms are valid, enforced in
    // irparse). This parser is lenient and round-trips whatever it sees.
    if line.starts_with("asm ") {
        let (template, after_idx) = parse_quoted_unescaped(line);
        let after = line[after_idx..].trim();
        let mut clobbers_memory = false;
        let rest: &str;
        // fast path: `memory` as first token after template
        if after.starts_with("memory") {
            let tail = &after["memory".len()..];
            if tail.is_empty() || tail.starts_with(|c: char| c.is_whitespace() || c == ',') {
                clobbers_memory = true;
                rest = tail
                    .trim_start_matches(|c: char| c.is_whitespace() || c == ',')
                    .trim();
                let mut operands = Vec::new();
                if !rest.is_empty() {
                    for tok in rest.split(',') {
                        let tok = tok.trim();
                        if tok.is_empty() {
                            continue;
                        }
                        let mut it = tok.split_whitespace();
                        let constraint = it.next().unwrap_or("").to_string();
                        let ptr = it.next().unwrap_or("").to_string();
                        if constraint.is_empty() || ptr.is_empty() {
                            panic!("malformed asm operand {tok:?} in {line:?}");
                        }
                        operands.push(AsmOperand { constraint, ptr });
                    }
                }
                return Inst::Asm(Asm {
                    template,
                    clobbers_memory,
                    operands,
                });
            } else {
                rest = after;
            }
        } else if after
            .split_whitespace()
            .any(|t| t.trim_matches(',') == "memory")
        {
            // `memory` appears later (e.g. `*m @x, memory`); remove it
            clobbers_memory = true;
            let parts: Vec<String> = after
                .split(',')
                .map(|s| {
                    s.split_whitespace()
                        .filter(|t| *t != "memory")
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|s| !s.trim().is_empty())
                .collect();
            let owned = parts.join(", ");
            let mut operands = Vec::new();
            if !owned.is_empty() {
                for tok in owned.split(',') {
                    let tok = tok.trim();
                    if tok.is_empty() {
                        continue;
                    }
                    let mut it = tok.split_whitespace();
                    let constraint = it.next().unwrap_or("").to_string();
                    let ptr = it.next().unwrap_or("").to_string();
                    if constraint.is_empty() || ptr.is_empty() {
                        panic!("malformed asm operand {tok:?} in {line:?}");
                    }
                    operands.push(AsmOperand { constraint, ptr });
                }
            }
            return Inst::Asm(Asm {
                template,
                clobbers_memory,
                operands,
            });
        } else {
            rest = after;
        }
        let mut operands = Vec::new();
        if !rest.is_empty() {
            for tok in rest.split(',') {
                let tok = tok.trim();
                if tok.is_empty() {
                    continue;
                }
                let mut it = tok.split_whitespace();
                let constraint = it.next().unwrap_or("").to_string();
                let ptr = it.next().unwrap_or("").to_string();
                if constraint.is_empty() || ptr.is_empty() {
                    panic!("malformed asm operand {tok:?} in {line:?}");
                }
                operands.push(AsmOperand { constraint, ptr });
            }
        }
        return Inst::Asm(Asm {
            template,
            clobbers_memory,
            operands,
        });
    }
    if line == "ret" {
        return Inst::Ret(None);
    }
    if let Some(rest) = line.strip_prefix("memcpy ") {
        let mut it = rest.split_whitespace();
        let dst = parse_val(it.next().unwrap());
        let src = parse_val(it.next().unwrap());
        let len_tok = it.next().unwrap();
        let len = if let Some(r) = len_tok.strip_prefix('%') {
            MemLen::Reg(Val::Reg(r.to_string()))
        } else {
            MemLen::Const(len_tok.parse().unwrap())
        };
        return Inst::Memcpy(Memcpy { dst, src, len });
    }
    if let Some(rest) = line.strip_prefix("store ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        return Inst::Store(Store {
            ty: parse_ty(parts[0]),
            val: parse_val(parts[1]),
            ptr: parts[2].to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix("ret ") {
        if rest == "void" {
            return Inst::Ret(None);
        }
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        return Inst::Ret(Some((t, parse_val(it.next().unwrap()))));
    }
    if let Some(rest) = line.strip_prefix("br ") {
        if let Some(r) = rest.strip_prefix("i1 ") {
            let mut it = r.split_whitespace();
            let cond = parse_val(it.next().unwrap());
            let t = it.next().unwrap().to_string();
            let f = it.next().unwrap().to_string();
            return Inst::BrCond(BrCond { cond, t, f });
        }
        return Inst::Br(Br {
            target: rest.to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix("call ") {
        let (ty, func, args, callees) = parse_call(rest);
        return Inst::Call(Call {
            dst: None,
            ty,
            func,
            args,
            callees,
        });
    }
    // defining instruction: %d = op ...
    let eq = line.find(" = ").unwrap();
    let dst = line[..eq].trim_start_matches('%').to_string();
    let body = line[eq + 3..].trim();
    if let Some(rest) = body.strip_prefix("load ") {
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        let ptr = it.next().unwrap().to_string();
        return Inst::Load(Load { dst, ty: t, ptr });
    }
    if let Some(rest) = body.strip_prefix("call ") {
        let (ty, func, args, callees) = parse_call(rest);
        return Inst::Call(Call {
            dst: Some(dst),
            ty,
            func,
            args,
            callees,
        });
    }
    if let Some(rest) = body.strip_prefix("alloca ") {
        let size = rest.trim().parse().unwrap();
        return Inst::Alloca(Alloca { dst, size });
    }
    if let Some(rest) = body.strip_prefix("inttoptr ") {
        let mut it = rest.split_whitespace();
        let from = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        assert_eq!(
            it.next().unwrap(),
            "to",
            "inttoptr must be '%d = inttoptr <t> <v> to <t2>'"
        );
        let to = parse_ty(it.next().unwrap());
        return Inst::IntToPtr(IntToPtr { dst, from, val, to });
    }
    if let Some(rest) = body.strip_prefix("zext ") {
        let mut it = rest.split_whitespace();
        let from = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        assert_eq!(
            it.next().unwrap(),
            "to",
            "zext must be '%d = zext <t> <v> to <t2>'"
        );
        let to = parse_ty(it.next().unwrap());
        return Inst::Zext(Zext { dst, from, val, to });
    }
    if let Some(rest) = body.strip_prefix("trunc ") {
        let mut it = rest.split_whitespace();
        let from = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        assert_eq!(
            it.next().unwrap(),
            "to",
            "trunc must be '%d = trunc <t> <v> to <t2>'"
        );
        let to = parse_ty(it.next().unwrap());
        return Inst::Trunc(Trunc { dst, from, val, to });
    }
    if let Some(rest) = body.strip_prefix("icmp ") {
        let mut it = rest.split_whitespace();
        let pred = it.next().unwrap().to_string();
        const PREDS: [&str; 10] = [
            "eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge",
        ];
        if !PREDS.contains(&pred.as_str()) {
            panic!("unsupported icmp predicate {pred}");
        }
        let t = parse_ty(it.next().unwrap());
        let a = parse_val(it.next().unwrap());
        let b = parse_val(it.next().unwrap());
        return Inst::Icmp(Icmp {
            dst,
            pred,
            ty: t,
            a,
            b,
        });
    }
    if let Some(rest) = body.strip_prefix("sext ") {
        let mut it = rest.split_whitespace();
        let from = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        assert_eq!(
            it.next().unwrap(),
            "to",
            "sext must be '%d = sext <t> <v> to <t2>'"
        );
        let to = parse_ty(it.next().unwrap());
        return Inst::Sext(Sext { dst, from, val, to });
    }
    if let Some(rest) = body.strip_prefix("select ") {
        let mut it = rest.split_whitespace();
        assert_eq!(
            it.next().unwrap(),
            "i1",
            "select must be '%d = select i1 <cond> <t> <a> <t> <b>'"
        );
        let cond = parse_val(it.next().unwrap());
        let t = it.next().unwrap();
        if t == "ptr" {
            let a = parse_val(it.next().unwrap());
            assert_eq!(it.next().unwrap(), "ptr", "select ptr arm type");
            let b = parse_val(it.next().unwrap());
            return Inst::Select(Select {
                dst,
                cond,
                ty: Ty::I16,
                a,
                b,
                ptr: true,
            });
        }
        let ty = parse_ty(t);
        let a = parse_val(it.next().unwrap());
        let t2 = parse_ty(it.next().unwrap());
        if ty != t2 {
            panic!("select operand type mismatch {ty:?} vs {t2:?}");
        }
        let b = parse_val(it.next().unwrap());
        return Inst::Select(Select {
            dst,
            cond,
            ty,
            a,
            b,
            ptr: false,
        });
    }
    if let Some(rest) = body.strip_prefix("phi ") {
        let mut it = rest.split_whitespace();
        let ty_tok = it.next().unwrap();
        // A pointer-typed phi prints with a `ptr` type token; parse_ty maps
        // that to Ty::I16, so the token is captured before the erasure.
        let ptr = ty_tok == "ptr";
        let t = parse_ty(ty_tok);
        let mut incoming = Vec::new();
        while let Some(v) = it.next() {
            let val = parse_val(v);
            let pred = it.next().unwrap().to_string();
            incoming.push((val, pred));
        }
        return Inst::Phi(Phi {
            dst,
            ty: t,
            ptr,
            incoming,
        });
    }
    if let Some(rest) = body.strip_prefix("gep ") {
        let (base, k, terms) = parse_gep_expr(rest);
        return Inst::Gep(Gep {
            dst,
            base,
            k,
            terms,
        });
    }
    if let Some(rest) = body.strip_prefix("freeze ") {
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        return Inst::Freeze(Freeze { dst, ty: t, val });
    }
    let mut it = body.split_whitespace();
    let op = it.next().unwrap();
    // float binops: `%d = fadd float %a %b` (f32 is implicit — both operands
    // and the dst are float).
    if matches!(op, "fadd" | "fsub" | "fmul" | "fdiv") {
        let t = parse_ty(it.next().unwrap());
        assert!(t == Ty::F32, "float binop must be f32, got {t:?}");
        let a = parse_val(it.next().unwrap());
        let b = parse_val(it.next().unwrap());
        let o = match op {
            "fadd" => FBinOp::FAdd,
            "fsub" => FBinOp::FSub,
            "fmul" => FBinOp::FMul,
            _ => FBinOp::FDiv,
        };
        return Inst::FloatBin(FloatBin { dst, op: o, a, b });
    }
    // fcmp: `%d = fcmp <pred> float %a %b` (dst is i1).
    if op == "fcmp" {
        let pred = it.next().unwrap().to_string();
        let t = parse_ty(it.next().unwrap());
        assert!(t == Ty::F32, "fcmp must be f32, got {t:?}");
        let a = parse_val(it.next().unwrap());
        let b = parse_val(it.next().unwrap());
        return Inst::Fcmp(Fcmp { dst, pred, a, b });
    }
    // conversions/casts: `%d = fptosi <from> <val> to <to>` etc.
    if matches!(
        op,
        "fptosi" | "fptoui" | "sitofp" | "uitofp" | "fpext" | "fptrunc"
    ) {
        let from = parse_ty(it.next().unwrap());
        let val = parse_val(it.next().unwrap());
        assert_eq!(
            it.next().unwrap(),
            "to",
            "{op} must be '%d = {op} <t> <v> to <t2>'"
        );
        let to = parse_ty(it.next().unwrap());
        let o = match op {
            "fptosi" => FloatConvOp::FpToSi,
            "fptoui" => FloatConvOp::FpToUi,
            "sitofp" => FloatConvOp::SiToFp,
            "uitofp" => FloatConvOp::UiToFp,
            "fpext" => FloatConvOp::Fpext,
            _ => FloatConvOp::Fptrunc,
        };
        return Inst::FloatConv(FloatConv {
            dst,
            op: o,
            from,
            val,
            to,
        });
    }
    let t = parse_ty(it.next().unwrap());
    let a = parse_val(it.next().unwrap());
    let b = parse_val(it.next().unwrap());
    let op = match op {
        "add" => BinOp::Add,
        "sub" => BinOp::Sub,
        "and" => BinOp::And,
        "or" => BinOp::Or,
        "xor" => BinOp::Xor,
        "mul" => BinOp::Mul,
        "udiv" => BinOp::UDiv,
        "urem" => BinOp::URem,
        "sdiv" => BinOp::SDiv,
        "srem" => BinOp::SRem,
        "shl" => BinOp::Shl,
        "lshr" => BinOp::LShr,
        "ashr" => BinOp::AShr,
        other => panic!("unsupported op {other}"),
    };
    Inst::Bin(Bin {
        dst,
        op,
        ty: t,
        a,
        b,
    })
}
