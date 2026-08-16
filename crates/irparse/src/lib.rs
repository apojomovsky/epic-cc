//! Parser for LLVM IR text (`.ll`) into the canonical `ir::Module`.
//!
//! Supports the integer-spine subset the PIC8 backend consumes: `load`/
//! `store` (global and SSA pointer operands), `add`/`sub`/`and`/`or`/`xor`,
//! `ret`, `zext`/`sext`/`trunc`, `icmp`, `select`, `br`/`brcond`, `call`,
//! `phi`, plus phase-3 pointers/const/structs: `getelementptr` (paren
//! byte-offset and multi-index forms), array/`constant` globals, named
//! struct types (`%struct.X = type {...}`), struct globals, `alloca`,
//! `llvm.memcpy`, and byval/sret call ABI params. Any other opcode, or any
//! structurally malformed input, panics loudly rather than silently
//! misparsing.

use std::collections::{HashMap, HashSet};

use ir::{Alloca, Bin, BinOp, Block, Br, BrCond, Call, CallArg, Func, Gep, GepBase, Global, Icmp, Inst, Load, Memcpy, Module, Param, Phi, Select, Sext, Store, Trunc, Ty, Val, Zext};

/// Strip LLVM parameter/return attributes we do not model, e.g.
/// `i16 noundef range(i16 -32768, 255) %1` -> `i16 %1`.
///
/// NOTE: this drops ALL tokens inside `range(...)`/`align(...)` paren
/// groups. It must NEVER be applied to a getelementptr — the paren GEP
/// `(i8, ptr @g, i16 2)` would be destroyed. GEPs are parsed from the raw
/// line by `parse_gep_expr`.
fn strip_attrs(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for tok in s.split_whitespace() {
        if depth > 0 {
            depth += tok.matches('(').count();
            depth -= tok.matches(')').count();
            continue;
        }
        if tok.starts_with("range(") || tok.starts_with("align(") {
            depth = 1 + tok.matches('(').count() - 1 - tok.matches(')').count();
            continue;
        }
        match tok {
            "noundef" | "nsw" | "nuw" | "nneg" | "samesign" | "volatile" | "tail" | "fastcc" | "inbounds"
            | "dso_local" | "local_unnamed_addr" | "internal" | "unnamed_addr" | "zeroext"
            | "signext" | "disjoint" | "nusw" | "inrange" => continue,
            _ => {}
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }
    out
}

fn ty_of(s: &str) -> Ty {
    match s {
        "i1" => Ty::I1,
        "i8" => Ty::I8,
        "i16" => Ty::I16,
        "i32" => Ty::I32,
        other => panic!("SPIKE: unsupported type {other:?}"),
    }
}

fn parse_val(s: &str) -> Val {
    let s = s.trim().trim_end_matches(',');
    if let Some(r) = s.strip_prefix('%') {
        Val::Reg(r.to_string())
    } else if let Some(g) = s.strip_prefix('@') {
        Val::Global(g.to_string())
    } else if s == "true" {
        Val::Const(1)
    } else if s == "false" {
        Val::Const(0)
    } else {
        Val::Const(s.parse::<i64>().unwrap_or_else(|_| panic!("SPIKE: cannot parse value {s:?}")))
    }
}

/// Decode an LLVM string literal `c"..."` into bytes. LLVM prints every
/// byte outside the printable range (plus `"` and `\`) as a `\XX` hex
/// escape, and a literal backslash byte 0x5C as the `\\` escape — so a
/// const table spanning the printable range contains `\\` runs, which the
/// hex-only decoder used to choke on.
fn parse_string_literal(s: &str) -> Vec<u8> {
    let start = s.find('"').unwrap() + 1;
    let end = start + s[start..].find('"').unwrap();
    let chars: Vec<char> = s[start..end].chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] == '\\' {
            out.push(b'\\');
            i += 2;
        } else if chars[i] == '\\' && i + 2 < chars.len() {
            let hi = chars[i + 1].to_digit(16).unwrap();
            let lo = chars[i + 2].to_digit(16).unwrap();
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(chars[i] as u8);
            i += 1;
        }
    }
    out
}

/// Split `s` on `sep` only at paren/bracket/brace depth 0 (so paren GEPs,
/// `initializes((0,1),(2,4))`, and array types are kept whole).
fn split_top_level<'a>(s: &'a str, sep: char) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            out.push(&s[start..i]);
            start = i + sep.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// Given `s` starting just after a `(`, return the content up to the
/// matching `)`.
fn balanced_inner(s: &str) -> Option<&str> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(&s[..i]);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Given `s` starting with `{`, return the content between the outer braces.
fn brace_inner(s: &str) -> Option<&str> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth == 1 {
                    return Some(&s[1..i]);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Tokenize `s`, keeping paren groups (`byval(%struct.S)`,
/// `initializes((0, 1), (2, 4))`) as single tokens.
fn tokenize_parens(s: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.trim().chars() {
        if c.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            continue;
        }
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

#[derive(Clone, Debug)]
struct StructInfo {
    size: u8,
    align: u8,
}
type StructTypes = HashMap<String, StructInfo>;

fn round_up(x: u16, align: u8) -> u16 {
    let a = u16::from(align);
    if a == 0 { x } else { ((x + a - 1) / a) * a }
}

/// Size and alignment of an LLVM type string, for structs already resolved.
fn ty_size_align(t: &str, types: &StructTypes) -> (u16, u8) {
    let t = t.trim();
    if let Some(r) = t.strip_prefix('[') {
        let close = r.find(']').unwrap();
        let inner = &r[..close]; // "N x T"
        let mut pit = inner.splitn(2, "x").map(|x| x.trim());
        let n: u16 = pit.next().unwrap().parse().unwrap();
        let elem = pit.next().unwrap();
        let (es, ea) = ty_size_align(elem, types);
        (n * es, ea)
    } else if let Some(n) = t.strip_prefix('%') {
        let info = types.get(n).unwrap_or_else(|| panic!("irparse: unknown struct type {t}"));
        (u16::from(info.size), info.align)
    } else {
        match t {
            "i1" | "i8" => (1, 1),
            "i16" => (2, 2),
            "i32" => (4, 2),
            other => panic!("irparse: unsupported type {other:?}"),
        }
    }
}

/// As `ty_size_align`, but `None` if a referenced struct is not resolved yet
/// (used for the fixpoint struct-table build).
fn ty_size_align_opt(t: &str, types: &StructTypes) -> Option<(u16, u8)> {
    let t = t.trim();
    if let Some(r) = t.strip_prefix('[') {
        let close = r.find(']').unwrap();
        let inner = &r[..close];
        let mut pit = inner.splitn(2, "x").map(|x| x.trim());
        let n: u16 = pit.next()?.parse().ok()?;
        let elem = pit.next()?;
        let (es, ea) = ty_size_align_opt(elem, types)?;
        Some((n * es, ea))
    } else if let Some(n) = t.strip_prefix('%') {
        types.get(n).map(|s| (u16::from(s.size), s.align))
    } else {
        match t {
            "i1" | "i8" => Some((1, 1)),
            "i16" => Some((2, 2)),
            "i32" => Some((4, 2)),
            _ => None,
        }
    }
}

/// Layout a struct from its field type strings. `None` while an unresolved
/// (mutually/forward-referenced) struct is referenced.
fn compute_struct(fields: &[String], types: &StructTypes) -> Option<StructInfo> {
    let mut off: u16 = 0;
    let mut max_align: u8 = 0;
    for f in fields {
        let (fsize, falign) = ty_size_align_opt(f, types)?;
        off = round_up(off, falign);
        assert!(off <= 255, "irparse: struct field offset {off} exceeds 255 (byte-addressed)");
        off += fsize;
        max_align = max_align.max(falign);
    }
    let size = round_up(off, max_align);
    assert!(size <= 255, "irparse: struct size {size} exceeds 255 (byte-addressed)");
    Some(StructInfo { size: size as u8, align: max_align })
}

/// Collect `%struct.X = type { ... }` declarations into a resolved size/
/// layout table (fixpoint over forward/recursive struct references).
fn build_struct_table(src: &str) -> StructTypes {
    let mut decls: Vec<(String, Vec<String>)> = Vec::new();
    for line in src.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("%struct.") {
            let eq = rest.find('=').unwrap();
            let name = format!("struct.{}", rest[..eq].trim());
            let ty_str = rest[eq + 1..].trim().strip_prefix("type ").expect("struct decl must be 'type {...}'");
            assert!(ty_str.starts_with('{'), "irparse: expected struct type, got {ty_str:?}");
            let inner = brace_inner(ty_str).expect("struct type must have balanced braces");
            let fields: Vec<String> = split_top_level(inner, ',').iter().map(|s| s.trim().to_string()).collect();
            decls.push((name, fields));
        }
    }
    let mut types: StructTypes = HashMap::new();
    loop {
        let mut changed = false;
        for (name, fields) in &decls {
            if types.contains_key(name) {
                continue;
            }
            if let Some(info) = compute_struct(fields, &types) {
                types.insert(name.clone(), info);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (name, _) in &decls {
        assert!(types.contains_key(name), "irparse: struct {name} unresolved (cycle?)");
    }
    types
}

/// Fresh-register generator for synthesized (materialized) GEP instructions.
/// Pre-seeded with every `%name` in the module so `__gep<N>` never collides.
struct Fresh {
    used: HashSet<String>,
    counter: usize,
}
impl Fresh {
    fn new(src: &str) -> Fresh {
        let mut used = HashSet::new();
        for line in src.lines() {
            let l = line.trim();
            let bytes = l.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.') {
                        j += 1;
                    }
                    if j > i + 1 {
                        used.insert(l[i + 1..j].to_string());
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
        Fresh { used, counter: 0 }
    }
    fn reg(&mut self) -> String {
        loop {
            self.counter += 1;
            let n = format!("__gep{}", self.counter);
            if !self.used.contains(&n) {
                self.used.insert(n.clone());
                return n;
            }
        }
    }
}

/// Parse one call/function pointer operand that may be an inlined GEP:
/// `ptr @g` / `ptr %r` -> a plain `Val`; `ptr getelementptr ...` ->
/// materialize a Gep inst and return its fresh reg.
fn parse_call_ptr_val(arg: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> Val {
    let b = arg.trim().strip_prefix("ptr").map(|x| x.trim()).unwrap_or(arg.trim());
    if b.contains("getelementptr") {
        let gpos = b.find("getelementptr").unwrap();
        let gsrc = &b[gpos + "getelementptr".len()..];
        let (base, k, terms) = parse_gep_expr(gsrc, types, fresh, out);
        let n = fresh.reg();
        out.push(Inst::Gep(Gep { dst: n.clone(), base, k, terms }));
        Val::Reg(n)
    } else {
        parse_val(b.split_whitespace().last().unwrap())
    }
}

/// Parse a load/store pointer operand (`ptr @g`, `ptr %r`, or an inlined
/// GEP that gets materialized as a fresh Gep inst). Returns `"@name"` or
/// `"%name"`.
fn parse_ptr_operand(arg: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> String {
    let b = arg.trim().strip_prefix("ptr").map(|x| x.trim()).unwrap_or(arg.trim());
    if b.starts_with("getelementptr") {
        let gsrc = &b["getelementptr".len()..];
        let (base, k, terms) = parse_gep_expr(gsrc, types, fresh, out);
        let n = fresh.reg();
        out.push(Inst::Gep(Gep { dst: n.clone(), base, k, terms }));
        format!("%{n}")
    } else {
        let tok = b.split_whitespace().next().unwrap();
        if tok.starts_with('@') {
            tok.to_string()
        } else {
            format!("%{}", tok.trim_start_matches('%'))
        }
    }
}

/// Compute the byte stride contributed by one GEP index and the type to
/// descend into. Level 0 treats the source type as an array-of-itself
/// (stride = its size); a scalar (i1/i8/i16) strides by its size with no
/// further descent. Struct-typed sources never reach here: `fold_gep`
/// rejects them up front.
fn stride_and_next(cur: &str, types: &StructTypes) -> (i64, String) {
    let cur = cur.trim();
    if cur.starts_with('[') {
        let close = cur.find(']').unwrap();
        let inner = &cur[1..close];
        let (sz, _) = ty_size_align(cur, types);
        let elem = inner.splitn(2, "x").nth(1).unwrap().trim().to_string();
        (i64::from(sz), elem)
    } else {
        // scalar (i1/i8/i16): stride = its size, no further descent
        let (sz, _) = ty_size_align(cur, types);
        (i64::from(sz), cur.to_string())
    }
}

/// Fold GEP indices into `(k, terms)`: constant indices × their stride fold
/// into the byte offset `k`; register indices become scaled `(scale, reg)`
/// terms.
fn fold_gep(source_ty: &str, index_parts: &[&str], types: &StructTypes) -> (u8, Vec<(u8, String)>) {
    let mut k: i64 = 0;
    let mut terms: Vec<(u8, String)> = Vec::new();
    let mut cur = source_ty.trim().to_string();
    for ip in index_parts {
        let ip = ip.trim();
        let idx = parse_val(ip.split_whitespace().last().unwrap());
        if cur.starts_with('%') {
            // A %struct.X source's first index is an array-of-struct element
            // index, not a field selector; real struct descent isn't
            // supported, so panic loudly instead of mis-folding.
            panic!("irparse: gep on struct-typed source {cur} unsupported (struct descent not implemented)");
        }
        let (stride, next) = stride_and_next(&cur, types);
        match &idx {
            Val::Const(c) => k += c * stride,
            Val::Reg(r) => terms.push((stride as u8, r.clone())),
            Val::Global(_) => panic!("irparse: gep index cannot be a global"),
        }
        cur = next;
    }
    assert!(k >= 0 && k <= 255, "irparse: gep byte offset {k} out of range");
    (k as u8, terms)
}

/// Parse a getelementptr into `(base, k, terms)`. Handles the paren
/// byte-offset form `(i8, ptr @g, i16 2)`, the multi-index form
/// `[4 x i8], ptr %1, i16 0, i16 %2`, scalar sources, reg/global bases, and
/// chained (inlined) bases. Strips its own `inbounds`/`nuw`/`nusw`/`inrange`
/// attrs. Runs on the RAW source (never `strip_attrs`).
fn parse_gep_expr(src: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> (GepBase, u8, Vec<(u8, String)>) {
    let mut s = src.trim();
    loop {
        let t = s.split_whitespace().next().unwrap_or("");
        if matches!(t, "inbounds" | "nuw" | "nusw" | "inrange") {
            s = s[t.len()..].trim();
        } else {
            break;
        }
    }
    let (source_ty, base_part, index_parts) = if let Some(rest) = s.strip_prefix('(') {
        let inner = balanced_inner(rest).unwrap_or_else(|| panic!("irparse: unbalanced gep parens in {src:?}"));
        let parts = split_top_level(inner, ',');
        (parts[0].trim().to_string(), parts[1].trim().to_string(), parts[2..].to_vec())
    } else {
        let parts = split_top_level(s, ',');
        (parts[0].trim().to_string(), parts[1].trim().to_string(), parts[2..].to_vec())
    };
    let base = parse_base(&base_part, types, fresh, out);
    let (k, terms) = fold_gep(&source_ty, &index_parts, types);
    (base, k, terms)
}

/// Parse a GEP base operand: `ptr @g`, `ptr %r`, or a chained inlined GEP
/// (materialized as a fresh Gep inst, base = its reg).
fn parse_base(base_part: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> GepBase {
    let b = base_part.trim().strip_prefix("ptr").map(|x| x.trim()).unwrap_or(base_part.trim());
    if let Some(g) = b.strip_prefix('@') {
        GepBase::Global(g.to_string())
    } else if let Some(r) = b.strip_prefix('%') {
        GepBase::Reg(r.to_string())
    } else if b.starts_with("getelementptr") {
        let inner_src = &b["getelementptr".len()..];
        let (ibase, ik, iterms) = parse_gep_expr(inner_src, types, fresh, out);
        let n = fresh.reg();
        out.push(Inst::Gep(Gep { dst: n.clone(), base: ibase, k: ik, terms: iterms }));
        GepBase::Reg(n)
    } else {
        panic!("irparse: bad gep base {base_part:?}");
    }
}

/// Parse one call argument (may carry an inlined GEP, byval, or sret).
fn parse_call_arg(a: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> CallArg {
    let a = a.trim();
    if a.contains("getelementptr") {
        let ty_tok = a.split_whitespace().next().unwrap();
        let ty = if ty_tok == "ptr" { None } else { Some(ty_of(ty_tok)) };
        let gpos = a.find("getelementptr").unwrap();
        let gsrc = &a[gpos + "getelementptr".len()..];
        let (base, k, terms) = parse_gep_expr(gsrc, types, fresh, out);
        let n = fresh.reg();
        out.push(Inst::Gep(Gep { dst: n.clone(), base, k, terms }));
        CallArg { ty, val: Val::Reg(n), byval: None, sret: false }
    } else {
        let toks = tokenize_parens(a);
        let mut ty = None;
        let mut byval = None;
        let mut sret = false;
        let mut val_tok = None;
        let mut skip_next = false;
        for t in &toks {
            if skip_next {
                skip_next = false;
                continue;
            }
            match t.as_str() {
                "ptr" => {}
                "i1" | "i8" | "i16" | "i32" => ty = Some(ty_of(t)),
                "align" => skip_next = true,
                "noundef" | "nonnull" | "noalias" | "nocapture" | "readonly" | "writeonly"
                | "writable" | "dead_on_unwind" | "immarg" | "zeroext" | "signext" => {}
                _ => {
                    if let Some(rest) = t.strip_prefix("byval(") {
                        let inner = rest.trim_end_matches(')');
                        let info = types.get(inner.trim_start_matches('%')).unwrap_or_else(|| panic!("irparse: unknown byval type {inner}"));
                        byval = Some(info.size);
                    } else if t.starts_with("sret(") {
                        sret = true;
                    } else if t.starts_with('%') || t.starts_with('@') {
                        val_tok = Some(t.clone());
                    } else if t.parse::<i64>().is_ok() {
                        val_tok = Some(t.clone());
                    }
                }
            }
        }
        CallArg { ty, val: parse_val(&val_tok.expect("call arg must carry a value")), byval, sret }
    }
}

/// Parse one function param: `ptr` (byval/sret/plain) or a scalar type, with
/// all LLVM attrs (`dead_on_unwind`, `noalias`, `nocapture`, `writable`,
/// `writeonly`, `readonly`, `nonnull`, `align`, `initializes(...)`, ...)
/// stripped. `byval(%X)`/`sret(%X)` sizes come from the struct table.
fn parse_param(p: &str, types: &StructTypes) -> Param {
    let toks = tokenize_parens(p);
    let mut scalar = None;
    let mut byval = None;
    let mut sret = false;
    let mut name = String::new();
    let mut skip_next = false;
    for t in &toks {
        if skip_next {
            skip_next = false;
            continue;
        }
        match t.as_str() {
            "ptr" | "dead_on_unwind" | "noalias" | "nocapture" | "writable" | "writeonly"
            | "readonly" | "nonnull" | "noundef" | "zeroext" | "signext" | "immarg" | "sret" | "byval" => {}
            "align" => skip_next = true,
            "i1" | "i8" | "i16" | "i32" => scalar = Some(ty_of(t)),
            _ => {
                if let Some(rest) = t.strip_prefix("byval(") {
                    let inner = rest.trim_end_matches(')');
                    let info = types.get(inner.trim_start_matches('%')).unwrap_or_else(|| panic!("irparse: unknown byval type {inner}"));
                    byval = Some(info.size);
                } else if t.starts_with("sret(") {
                    sret = true;
                } else if t.starts_with('%') {
                    name = t.trim_start_matches('%').to_string();
                } else if t.starts_with("initializes")
                    || t.starts_with("range(")
                    || t.starts_with("align(")
                    || t.starts_with('#')
                {
                    // paren group / attr-group ref
                } else {
                    panic!("irparse: unsupported param type token {t:?} in {p:?}");
                }
            }
        }
    }
    let width = if let Some(b) = byval {
        b
    } else if sret {
        2
    } else if let Some(t) = scalar {
        t.bytes()
    } else {
        2 // plain ptr param: a 2-byte address slot
    };
    Param { name, width, byval, sret }
}

/// Parse `.ll` text into canonical IR.
pub fn parse_ll(src: &str) -> Module {
    let types = build_struct_table(src);
    let mut fresh = Fresh::new(src);

    let mut globals = Vec::new();
    let mut funcs = Vec::new();
    let mut lines = src.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('!') {
            continue;
        }

        // Global definitions: "@name = ... global|constant <ty> ..."
        if line.starts_with('@') {
            let eq = line.find('=').unwrap();
            let name = line[1..eq].trim().to_string();
            let after = line[eq + 1..].trim();
            let (is_const, rest) = if let Some(i) = after.find("global ") {
                (false, &after[i + "global ".len()..])
            } else if let Some(i) = after.find("constant ") {
                (true, &after[i + "constant ".len()..])
            } else {
                continue; // not a global definition we care about
            };
            let rest = rest.trim();
            let (ty, size, bytes) = if rest.starts_with('[') {
                // array global: "[N x i8] zeroinitializer" / "[N x i8] c\"...\""
                let close = rest.find(']').unwrap();
                let inner = &rest[1..close]; // e.g. "8 x i8"
                let mut pit = inner.split('x').map(|s| s.trim());
                let n: usize = pit.next().unwrap().parse().unwrap();
                let elem = ty_of(pit.next().unwrap());
                let size = n * elem.bytes() as usize;
                // Const (flash) tables may span two 256-byte chunks (<= 511
                // bytes); RAM globals are byte-addressed, so they stay <= 255.
                if is_const {
                    assert!(size <= 511, "irparse: const array @{name} too large ({size} bytes; max 511)");
                } else {
                    assert!(size <= 255, "irparse: array @{name} too large ({size} bytes)");
                }
                let size = size as u16;
                let init = rest[close + 1..].trim();
                let bytes = if init.starts_with("zeroinitializer") {
                    vec![0u8; size as usize]
                } else if init.starts_with("c\"") {
                    parse_string_literal(init)
                } else {
                    panic!("SPIKE LIMIT: array global initializer {init:?}");
                };
                (elem, size, bytes)
            } else if let Some(struct_tok) = rest.split_whitespace().next().filter(|t| t.starts_with('%')) {
                // struct global: "%struct.S zeroinitializer, align 2" — size
                // from the type table, bytes = zeros.
                let info = types.get(struct_tok.trim_start_matches('%')).unwrap_or_else(|| panic!("irparse: unknown struct type {struct_tok} for @{name}"));
                let size = u16::from(info.size);
                let init = rest[struct_tok.len()..].trim().split(',').next().unwrap().trim();
                let bytes = if init.starts_with("zeroinitializer") {
                    vec![0u8; size as usize]
                } else {
                    panic!("SPIKE LIMIT: struct global initializer {init:?}");
                };
                (Ty::I8, size, bytes)
            } else {
                // scalar global: "<ty> <init>[, align N]" -> type is the first token
                let ty = ty_of(rest.split_whitespace().next().unwrap());
                (ty, u16::from(ty.bytes()), Vec::new())
            };
            globals.push(Global { name, ty, is_const, size, bytes, addr: None });
            continue;
        }

        if line.starts_with("define") {
            let at = line.find('@').unwrap();
            let open = line[at..].find('(').unwrap() + at;
            let name = line[at + 1..open].trim().to_string();
            let params_str = balanced_inner(&line[open + 1..]).unwrap();
            // Return type: strip attrs from everything before @name; the
            // last token is the type (zeroext/signext returns stripped here).
            let head = strip_attrs(&line[..at]);
            let ret_tok = head.split_whitespace().last().unwrap().to_string();
            let ret = if ret_tok == "void" { None } else { Some(ty_of(&ret_tok)) };

            let mut params = Vec::new();
            for p in split_top_level(params_str, ',') {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                params.push(parse_param(p, &types));
            }

            let mut blocks: Vec<Block> = vec![Block { label: "0".into(), insts: Vec::new() }];
            for raw in lines.by_ref() {
                let l = raw.trim();
                if l == "}" {
                    break;
                }
                if l.is_empty() || l.starts_with(';') {
                    continue;
                }
                if let Some(colon) = l.find(':') {
                    let head = &l[..colon];
                    if !head.is_empty()
                        && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                        && !l.starts_with('%')
                    {
                        blocks.push(Block { label: head.to_string(), insts: Vec::new() });
                        continue;
                    }
                }
                let insts = parse_inst(l, &types, &mut fresh);
                blocks.last_mut().unwrap().insts.extend(insts);
            }
            funcs.push(Func { name, ret, params, blocks });
        }
    }
    Module { globals, funcs }
}

/// Parse a single `.ll` instruction (RAW line; GEPs are never attr-stripped).
/// Returns a `Vec` because inlined GEP operands materialize a synthetic Gep
/// inst before the consuming instruction, and `llvm.lifetime.*` calls
/// produce nothing.
fn parse_inst(line: &str, types: &StructTypes, fresh: &mut Fresh) -> Vec<Inst> {
    // Drop trailing metadata: ", !tbaa !2" / ", !llvm.loop !5"
    let line = match line.find(", !") {
        Some(i) => &line[..i],
        None => line,
    };
    let trimmed = line.trim();
    let (dst, rest) = match trimmed.find(" = ") {
        Some(i) => (Some(trimmed[..i].trim_start_matches('%').to_string()), trimmed[i + 3..].trim()),
        None => (None, trimmed),
    };

    // A defining or standalone `tail/fastcc call` carries its markers before
    // the opcode; strip them so `call` is detected.
    let mut rest = rest;
    loop {
        if let Some(r) = rest.strip_prefix("tail ") {
            rest = r.trim();
        } else if let Some(r) = rest.strip_prefix("fastcc ") {
            rest = r.trim();
        } else {
            break;
        }
    }

    let op = rest.split_whitespace().next().unwrap().to_string();
    let mut out = Vec::new();
    match op.as_str() {
        "getelementptr" => {
            let src = rest["getelementptr".len()..].trim();
            let (base, k, terms) = parse_gep_expr(src, types, fresh, &mut out);
            out.push(Inst::Gep(Gep { dst: dst.unwrap(), base, k, terms }));
        }
        "alloca" => {
            let after = rest["alloca".len()..].trim();
            let ty_tok = after.split(',').next().unwrap().trim();
            let size = if let Some(n) = ty_tok.strip_prefix('%') {
                types.get(n).unwrap_or_else(|| panic!("irparse: unknown alloca type {ty_tok}")).size
            } else {
                ty_of(ty_tok).bytes()
            };
            out.push(Inst::Alloca(Alloca { dst: dst.unwrap(), size }));
        }
        "load" => {
            let args = split_top_level(&rest["load".len()..], ',');
            let ty = ty_of(strip_attrs(args[0]).trim());
            let ptr = parse_ptr_operand(args[1], types, fresh, &mut out);
            out.push(Inst::Load(Load { dst: dst.unwrap(), ty, ptr }));
        }
        "store" => {
            let args = split_top_level(&rest["store".len()..], ',');
            let a0 = strip_attrs(args[0]);
            let mut it = a0.trim().split_whitespace();
            let ty = ty_of(it.next().unwrap());
            let val = parse_val(it.next().unwrap());
            let ptr = parse_ptr_operand(args[1], types, fresh, &mut out);
            out.push(Inst::Store(Store { ty, val, ptr }));
        }
        "call" => {
            let body = rest["call".len()..].trim();
            let open = body.find('(').unwrap();
            let head = &body[..open];
            let func = head.split_whitespace().last().unwrap().trim_start_matches('@').to_string();
            let args_str = balanced_inner(&body[open + 1..]).unwrap();
            if func.starts_with("llvm.memcpy.p0.p0") {
                let a = split_top_level(args_str, ',');
                let dst = parse_call_ptr_val(a[0], types, fresh, &mut out);
                let src = parse_call_ptr_val(a[1], types, fresh, &mut out);
                // const-len <= 255 assert + non-const-len panic: len is u8
                let len: u8 = a[2].split_whitespace().last().unwrap().parse().expect("irparse: memcpy len must be a const u8 <= 255");
                // isvolatile (a[3] = `i1 true`/`i1 false`) is an LLVM
                // optimization hint; our byte copy is identical either way.
                out.push(Inst::Memcpy(Memcpy { dst, src, len }));
            } else if func.starts_with("llvm.lifetime.start") || func.starts_with("llvm.lifetime.end") {
                // lifetime markers produce no IR
            } else {
                let arg_parts = split_top_level(args_str, ',');
                let mut args = Vec::new();
                for ap in arg_parts {
                    let ap = ap.trim();
                    if ap.is_empty() {
                        continue;
                    }
                    args.push(parse_call_arg(ap, types, fresh, &mut out));
                }
                let first = head.trim();
                let at = first.rfind('@').unwrap();
                let ret_tok = first[..at].trim().split_whitespace().last().unwrap().to_string();
                let ty = if ret_tok == "void" { None } else { Some(ty_of(&ret_tok)) };
                out.push(Inst::Call(Call { dst, ty, func, args }));
            }
        }
        "br" => {
            let body = rest["br".len()..].trim();
            if body.starts_with("label") {
                out.push(Inst::Br(Br { target: body.split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string() }));
            } else {
                let parts = split_top_level(body, ',');
                let cond = parse_val(parts[0].split_whitespace().nth(1).unwrap());
                let t = parts[1].split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string();
                let f = parts[2].split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string();
                out.push(Inst::BrCond(BrCond { cond, t, f }));
            }
        }
        "ret" => {
            let body = rest["ret".len()..].trim();
            if body == "void" {
                out.push(Inst::Ret(None));
            } else {
                let mut it = body.split_whitespace();
                let ty = ty_of(it.next().unwrap());
                out.push(Inst::Ret(Some((ty, parse_val(it.next().unwrap())))));
            }
        }
        "phi" => {
            let body = rest["phi".len()..].trim();
            let ty = ty_of(body.split_whitespace().next().unwrap());
            let mut incoming = Vec::new();
            for part in body.split('[').skip(1) {
                let inner = part.split(']').next().unwrap();
                let mut it = inner.split(',');
                let v = parse_val(it.next().unwrap());
                let lbl = it.next().unwrap().trim().trim_start_matches('%').to_string();
                incoming.push((v, lbl));
            }
            out.push(Inst::Phi(Phi { dst: dst.unwrap(), ty, incoming }));
        }
        "zext" | "sext" | "trunc" => {
            let body = strip_attrs(&rest[op.len()..]);
            let to_i = body.rfind(" to ").unwrap();
            let (lhs, rhs) = (body[..to_i].trim(), body[to_i + 4..].trim());
            let mut it = lhs.split_whitespace();
            let from = ty_of(it.next().unwrap());
            let val = parse_val(it.next().unwrap());
            let to = ty_of(rhs);
            match op.as_str() {
                "zext" => out.push(Inst::Zext(Zext { dst: dst.unwrap(), from, val, to })),
                "sext" => out.push(Inst::Sext(Sext { dst: dst.unwrap(), from, val, to })),
                _ => out.push(Inst::Trunc(Trunc { dst: dst.unwrap(), from, val, to })),
            }
        }
        "icmp" => {
            let body = strip_attrs(&rest["icmp".len()..]);
            let mut it = body.split_whitespace();
            let pred = it.next().unwrap().to_string();
            const PREDS: [&str; 10] = ["eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge"];
            if !PREDS.contains(&pred.as_str()) { panic!("SPIKE: unsupported icmp predicate {pred:?} in line: {line}"); }
            let ty = ty_of(it.next().unwrap());
            let a = parse_val(it.next().unwrap());
            let b = parse_val(it.next().unwrap());
            out.push(Inst::Icmp(Icmp { dst: dst.unwrap(), pred, ty, a, b }));
        }
        "select" => {
            let body = rest["select".len()..].trim();
            let parts = split_top_level(body, ',');
            let cond = parse_val(parts[0].split_whitespace().nth(1).unwrap());
            let mut it1 = parts[1].split_whitespace();
            let ty = ty_of(it1.next().unwrap());
            let a = parse_val(it1.next().unwrap());
            let b = parse_val(parts[2].split_whitespace().nth(1).unwrap());
            out.push(Inst::Select(Select { dst: dst.unwrap(), cond, ty, a, b }));
        }
        "add" | "and" | "or" | "xor" | "sub" | "mul" | "udiv" | "urem" | "sdiv" | "srem"
        | "shl" | "lshr" | "ashr" => {
            let body = strip_attrs(&rest[op.len()..]);
            let mut it = body.split_whitespace();
            let ty = ty_of(it.next().unwrap());
            let a = parse_val(it.next().unwrap());
            let b = parse_val(it.next().unwrap());
            let o = match op.as_str() {
                "add" => BinOp::Add,
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
                _ => BinOp::Sub,
            };
            out.push(Inst::Bin(Bin { dst: dst.unwrap(), op: o, ty, a, b }));
        }
        "freeze" => {
            let body = strip_attrs(&rest[op.len()..]);
            let mut it = body.split_whitespace();
            let ty = ty_of(it.next().unwrap());
            let val = parse_val(it.next().unwrap());
            out.push(Inst::Freeze(ir::Freeze { dst: dst.unwrap(), ty, val }));
        }
        other => panic!("SPIKE LIMIT: unsupported opcode {other:?} in line: {line}"),
    }
    out
}
