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

use ir::{Alloca, Bin, BinOp, Block, Br, BrCond, Call, CallArg, FBinOp, Fcmp, FloatBin, FloatConv, FloatConvOp, Func, Gep, GepBase, Global, Icmp, Inst, Load, Memcpy, MemLen, Module, Param, Phi, Select, Sext, Store, Trunc, Ty, Val, Zext};

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
        "float" | "f32" => Ty::F32,
        other => panic!("SPIKE: unsupported type {other:?}"),
    }
}

fn parse_val(s: &str) -> Val {
    parse_val_typed(s, None)
}

/// Type-aware constant parse. For a `float` operand clang prints constants
/// that cannot be represented in 8 hex digits as their DOUBLE-precision
/// promotion — `store volatile float 0x3FB99999A0000000` is the f64 bit
/// pattern of 0.1f, NOT a 64-bit integer to truncate (the M15 float
/// differential found the old low-32-bits truncation storing 0xA0000000
/// instead of 0x3DCCCCCD). A >8-digit hex on an f32 operand is converted
/// back to the f32 bit pattern; the 8-digit hex form (`0x3F800000`) is
/// already the f32 bits; non-float types keep the full integer.
fn parse_val_typed(s: &str, ty: Option<Ty>) -> Val {
    let s = s.trim().trim_end_matches(',');
    if let Some(r) = s.strip_prefix('%') {
        Val::Reg(r.to_string())
    } else if let Some(g) = s.strip_prefix('@') {
        Val::Global(g.to_string())
    } else if s == "true" {
        Val::Const(1)
    } else if s == "false" {
        Val::Const(0)
    } else if s == "poison" {
        // A `poison` operand is never observed: any instruction that
        // consumes poison has undefined behavior, so a conforming program
        // cannot read the value. clang -O1 emits `poison` for ordinary
        // programs (e.g. a dead call arg the optimizer replaced after
        // specializing a noinline helper). Any materialization is therefore
        // correct; 0 keeps the IR pipeline simple.
        Val::Const(0)
    } else if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let v = u64::from_str_radix(hex, 16)
            .unwrap_or_else(|_| panic!("SPIKE: cannot parse hex value {s:?}"));
        if ty == Some(Ty::F32) && hex.len() > 8 {
            Val::Const((f64::from_bits(v) as f32).to_bits() as i64)
        } else {
            Val::Const(v as i64)
        }
    } else if let Ok(k) = s.parse::<i64>() {
        Val::Const(k)
    } else if let Ok(f) = s.parse::<f32>() {
        // Decimal f32 constant materialized as its 32-bit bit pattern
        // (e.g. `1.000000e+00` -> 0x3F800000).
        Val::Const(f.to_bits() as i64)
    } else {
        panic!("SPIKE: cannot parse value {s:?}")
    }
}

/// Decode a typed element-list initializer (`[i16 4660, i16 -25924]`,
/// `[float 0x3FB99999A0000000, float 5.000000e-01]`) into the table's
/// little-endian byte blob. clang -O1 prints const arrays of multi-byte
/// elements this way — never as `c"..."` — so each element is decoded with
/// the same value grammar as operands (`parse_val_typed` handles the f64
/// promotion clang prints for float constants that do not fit 8 hex
/// digits) and appended little-endian. The element type's byte width is
/// the stride.
fn parse_array_elements(init: &str, elem: Ty) -> Vec<u8> {
    let inner = init
        .strip_prefix('[')
        .and_then(|s| matching_bracket(&init).map(|i| &s[..i.saturating_sub(1)]))
        .unwrap_or_else(|| panic!("SPIKE LIMIT: array global initializer {init:?}"));
    let width = elem.bytes() as usize;
    let mut out = Vec::new();
    for elt in split_top_level(inner, ',') {
        let elt = elt.trim();
        if elt.is_empty() {
            continue;
        }
        // Element tokens are type-prefixed (`i16 4660`, `float 0x...`);
        // the type must match the array's element type.
        let (val_tok, elt_ty) = match elt.split_once(' ') {
            Some((t, v)) => (v.trim(), ty_of(t)),
            None => (elt, elem),
        };
        assert_eq!(
            elt_ty, elem,
            "SPIKE LIMIT: array element type mismatch: {elt:?} (array element is {elem:?})"
        );
        let v = match parse_val_typed(val_tok, Some(elem)) {
            Val::Const(k) => k,
            _ => panic!("SPIKE LIMIT: array global initializer element {elt:?}"),
        };
        let uv = v as u64;
        for i in 0..width {
            out.push(((uv >> (8 * i)) & 0xFF) as u8);
        }
    }
    out
}

/// Size/alignment of a literal struct type string (`{ i8, i8, i16 }`,
/// nested `{ ... }` fields, `[N x T]` fields) — same layout rules as
/// `compute_struct`: fields at aligned offsets, size rounded up to the max
/// field alignment. `types` resolves any named `%struct.X` field.
fn literal_ty_size_align(t: &str, types: &StructTypes) -> (u16, u8) {
    let t = t.trim();
    let inner = brace_inner(t).expect("literal struct type must be `{ ... }`");
    let mut off: u16 = 0;
    let mut max_align: u8 = 0;
    for f in split_top_level(inner, ',') {
        let f = f.trim();
        if f.is_empty() {
            continue;
        }
        let (fsize, falign) = ty_size_align(f, types);
        off = round_up(off, falign);
        off += fsize;
        max_align = max_align.max(falign);
    }
    (round_up(off, max_align), max_align)
}

/// Strip a clang self-type prefix from a nested value. clang prints every
/// non-scalar initializer with its own type: `{ T } { v }` for struct
/// values, `[N x T] c"..."` / `[N x T] [...]` for array values. The value's
/// first brace/bracket group is that self-type — strip it when present so
/// the remainder is the bare value the decoder expects.
fn strip_self_type<'a>(ty: &str, value: &'a str) -> &'a str {
    let value = value.trim();
    if value.starts_with('{') {
        // `{ i8, i8, i16 } { i8 66, ... }` — the first brace group is the
        // self-type; `brace_inner` stops at its closing brace, and the
        // remainder starts with the value's own `{`.
        let inner = brace_inner(value).unwrap_or("");
        let rest = value[inner.len() + 2..].trim();
        return if rest.starts_with('{') { rest } else { value };
    }
    if ty.trim().starts_with('[') && value.starts_with('[') {
        // `[3 x i8] c"abc"` / `[2 x T] [ ... ]` — the first bracket group
        // is the self-type.
        if let Some(i) = matching_bracket(value) {
            let rest = value[i + 1..].trim();
            if rest.starts_with('c') || rest.starts_with('[') {
                return rest;
            }
        }
    }
    value
}

/// Decode one constant of a literal/named type into its flat little-endian
/// byte blob. `value` forms: `zeroinitializer`, a scalar (`i8 65`,
/// `i16 4660`, `float 0x...`), a `c"..."` or `[...]` array value, or a
/// nested `{ ... }` struct value (possibly self-type-prefixed). Unknown
/// shapes panic loudly.
fn decode_typed_value(ty: &str, value: &str, types: &StructTypes) -> Vec<u8> {
    let ty = ty.trim();
    let value = strip_self_type(ty, value).trim();
    if value.starts_with("zeroinitializer") {
        let (size, _) = ty_size_align(ty, types);
        return vec![0u8; size as usize];
    }
    if let Some(inner) = ty.strip_prefix('[') {
        // Array value: `c"..."` (i8), a `[...]` element list, or (struct
        // elements) a brace list.
        let close = ty.find(']').unwrap();
        let inner = &ty[1..close];
        let mut pit = inner.splitn(2, "x").map(|s| s.trim());
        let n: usize = pit.next().unwrap().parse().unwrap();
        let elem = pit.next().unwrap();
        let bytes = if elem == "i8" && value.starts_with('c') && value.contains('"') {
            parse_string_literal(value)
        } else if value.starts_with('[') {
            if elem.starts_with('{') {
                let inner_list = value
                    .strip_prefix('[')
                    .and_then(|s| matching_bracket(value).map(|i| &s[..i.saturating_sub(1)]))
                    .unwrap_or_else(|| panic!("SPIKE LIMIT: struct array value {value:?}"));
                let mut out = Vec::new();
                for elt in split_top_level(inner_list, ',') {
                    let elt = elt.trim();
                    if elt.is_empty() {
                        continue;
                    }
                    out.extend(decode_typed_value(elem, elt, types));
                }
                out
            } else {
                parse_array_elements(value, ty_of(elem))
            }
        } else {
            panic!("SPIKE LIMIT: array value {value:?} for type {ty:?}");
        };
        let expect = n * usize::from(ty_size_align(elem, types).0);
        assert_eq!(
            bytes.len(),
            expect,
            "SPIKE LIMIT: array value {value:?} decodes to {} bytes, expected {expect} for {ty:?}",
            bytes.len()
        );
        return bytes;
    }
    if ty.starts_with('{') {
        return decode_literal_struct(ty, value, types);
    }
    // Scalar value: `i8 65`, `i16 -5`, `float 1.500000e+00`.
    let (_, v) = value.split_once(' ').unwrap_or(("", value));
    let w = ty_of(ty).bytes() as usize;
    match parse_val_typed(v.trim(), Some(ty_of(ty))) {
        Val::Const(k) => {
            let uv = k as u64;
            (0..w).map(|i| ((uv >> (8 * i)) & 0xFF) as u8).collect()
        }
        _ => panic!("SPIKE LIMIT: non-constant struct field value {value:?}"),
    }
}

/// Decode a literal struct initializer (`{ i8 65, i8 0, i16 4660 }`) into
/// the flat little-endian blob, placing each field at its aligned offset
/// (the same rules as the type table). clang prints every field including
/// padding, but missing trailing fields decode as zeros for robustness.
fn decode_literal_struct(ty: &str, init: &str, types: &StructTypes) -> Vec<u8> {
    let inner = brace_inner(ty).expect("literal struct type must be `{ ... }`");
    let ty_fields: Vec<&str> = split_top_level(inner, ',').into_iter().map(|s| s.trim()).collect();
    let (size, _) = literal_ty_size_align(ty, types);
    let mut blob = vec![0u8; size as usize];
    if init.starts_with("zeroinitializer") {
        return blob;
    }
    let v_inner = brace_inner(init)
        .unwrap_or_else(|| panic!("SPIKE LIMIT: struct initializer {init:?} for type {ty:?}"));
    let values: Vec<&str> = split_top_level(v_inner, ',').into_iter().map(|s| s.trim()).collect();
    assert!(
        values.len() <= ty_fields.len(),
        "SPIKE LIMIT: struct initializer {init:?} has more values than type {ty:?} has fields"
    );
    let mut off: u16 = 0;
    for (i, f) in ty_fields.iter().enumerate() {
        if f.is_empty() {
            continue;
        }
        let (fsize, falign) = ty_size_align(f, types);
        off = round_up(off, falign);
        if let Some(v) = values.get(i) {
            if !v.is_empty() {
                let fbytes = decode_typed_value(f, v, types);
                assert_eq!(
                    fbytes.len(),
                    fsize as usize,
                    "SPIKE LIMIT: field value {v:?} of {f:?} decodes to the wrong width"
                );
                blob[off as usize..(off + fsize) as usize].copy_from_slice(&fbytes);
            }
        }
        off += fsize;
    }
    blob
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

/// Given `s` starting with `[`, return the index of the matching `]`.
fn matching_bracket(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
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
    } else if t.starts_with('{') {
        // Literal struct type (`{ i8, i8, i16 }`): layout by the same
        // rules as `compute_struct`. Literal structs are value types
        // (no cycles), so plain recursion terminates.
        literal_ty_size_align(t, types)
    } else {
        match t {
            "i1" | "i8" => (1, 1),
            "i16" => (2, 2),
            "i32" | "float" | "f32" => (4, 2),
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
            "i32" | "float" | "f32" => Some((4, 2)),
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

/// Parse a load/store pointer operand (`ptr @g`, `ptr %r`, an inlined GEP
/// that gets materialized as a fresh Gep inst, or an `inttoptr (<ty> <k> to
/// ptr)` constant pointer). Returns `"@name"`, `"%name"`, or the literal ptr
/// form `"0x<K>"` (SFR access — distinct from `@global`/`%reg`).
fn parse_ptr_operand(arg: &str, types: &StructTypes, fresh: &mut Fresh, out: &mut Vec<Inst>) -> String {
    let b = arg.trim().strip_prefix("ptr").map(|x| x.trim()).unwrap_or(arg.trim());
    if b.starts_with("inttoptr") {
        // inttoptr (<ty> <k> to ptr) -> literal ptr form "0x<K>"
        let open = b.find('(').unwrap_or_else(|| panic!("irparse: malformed inttoptr {b:?}"));
        let inner = balanced_inner(&b[open + 1..]).unwrap_or_else(|| panic!("irparse: unbalanced inttoptr parens in {b:?}"));
        let mut prev = "";
        let mut k = None;
        for t in inner.split_whitespace() {
            if t == "to" {
                k = Some(prev);
                break;
            }
            prev = t;
        }
        let k: u8 = k
            .unwrap_or_else(|| panic!("irparse: malformed inttoptr {b:?}"))
            .parse()
            .unwrap_or_else(|_| panic!("irparse: inttoptr address not a byte constant: {b:?}"));
        format!("0x{k:02x}")
    } else if b.starts_with("getelementptr") {
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
    // True when the previous GEP level was an array — a struct cur is then
    // the array's ELEMENT type, so this index is an element selector
    // (stride = struct size). Without it, a struct index would be a FIELD
    // selector (struct descent, out of scope — panic).
    let mut from_array = false;
    for ip in index_parts {
        let ip = ip.trim();
        let idx = parse_val(ip.split_whitespace().last().unwrap());
        if cur.starts_with('%') || cur.starts_with('{') {
            if from_array {
                // `[N x %struct.S], i16 0, i16 %i` — the index after an
                // array-of-struct descent strides by sizeof(%struct.S)
                // (LLVM: the second index indexes the array's element
                // type). clang -O1 emits this for `&CARR[i]`; field
                // selection rides as a separate i8-offset GEP.
                let (sz, _) = ty_size_align(&cur, types);
                match &idx {
                    Val::Const(c) => k += c * i64::from(sz),
                    Val::Reg(r) => terms.push((sz as u8, r.clone())),
                    Val::Global(_) => panic!("irparse: gep index cannot be a global"),
                }
                // The element's type is the struct itself: a further index
                // would be a field selector — reject it.
                from_array = false;
                continue;
            }
            // A struct source's first index IS its element selector; the
            // next would be a field selector. Neither is supported (field
            // descent is out of scope) — panic loudly instead of
            // mis-folding.
            panic!("irparse: gep on struct-typed source {cur} unsupported (struct descent not implemented)");
        }
        let (stride, next) = stride_and_next(&cur, types);
        from_array = cur.starts_with('[');
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
                "i1" | "i8" | "i16" | "i32" | "float" | "f32" => ty = Some(ty_of(t)),
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
                    } else if t == "poison" {
                        // see parse_val: a poison arg is never observed, so
                        // materializing it as Const(0) is sound.
                        val_tok = Some(t.clone());
                    } else if t.parse::<i64>().is_ok() {
                        val_tok = Some(t.clone());
                    } else if t.parse::<f32>().is_ok() || t.starts_with("0x") || t.starts_with("0X") {
                        // an f32 constant (decimal `5.000000e-01` or hex bit
                        // pattern `0x3F800000`) — parse_val materializes the bits.
                        val_tok = Some(t.clone());
                    }
                }
            }
        }
        CallArg { ty: ty.clone(), val: parse_val_typed(&val_tok.expect("call arg must carry a value"), ty), byval, sret }
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
            "i1" | "i8" | "i16" | "i32" | "float" | "f32" => scalar = Some(ty_of(t)),
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
            // LLVM bookkeeping globals (`@llvm.used`, `@llvm.compiler.used`,
            // ...) carry `[N x ptr]` types we do not model — clang emits
            // them for address-taken symbols like the interrupt handler.
            // They are metadata for the backend, never PIC8 data, so skip
            // them like the `llvm.lifetime.*` call skip.
            if name.starts_with("llvm.") {
                continue;
            }
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
                // / "[N x { ... }] { {...}, {...} }" (array of literal
                // structs — clang's expanded form for struct arrays)
                let close = rest.find(']').unwrap();
                let inner = &rest[1..close]; // e.g. "8 x i8"
                let mut pit = inner.split('x').map(|s| s.trim());
                let n: usize = pit.next().unwrap().parse().unwrap();
                let elem_str = pit.next().unwrap();
                if elem_str.starts_with('{') {
                    // "[N x { ... }] { ... }" — decode each element through
                    // the literal-struct decoder.
                    let (es, _) = literal_ty_size_align(elem_str, &types);
                    let size = n * es as usize;
                    if is_const {
                        assert!(size <= 65535, "irparse: const array @{name} too large ({size} bytes; max 65535)");
                    } else {
                        assert!(size <= 255, "irparse: array @{name} too large ({size} bytes)");
                    }
                    let size = size as u16;
                    let init = rest[close + 1..].trim();
                    let bytes = if init.starts_with("zeroinitializer") {
                        vec![0u8; size as usize]
                    } else {
                        decode_typed_value(&rest[..close + 1], init, &types)
                    };
                    (Ty::I8, size, bytes)
                } else {
                    let elem = ty_of(elem_str);
                    let size = n * elem.bytes() as usize;
                    // Const (flash) tables may span any number of 256-byte
                    // chunks (up to the 16-bit index space, 65535 bytes — the
                    // device flash bound is enforced later by the assembler);
                    // RAM globals are byte-addressed, so they stay <= 255.
                    if is_const {
                        assert!(size <= 65535, "irparse: const array @{name} too large ({size} bytes; max 65535)");
                    } else {
                        assert!(size <= 255, "irparse: array @{name} too large ({size} bytes)");
                    }
                    let size = size as u16;
                    let init = rest[close + 1..].trim();
                    let bytes = if init.starts_with("zeroinitializer") {
                        vec![0u8; size as usize]
                    } else if init.starts_with("c\"") {
                        parse_string_literal(init)
                    } else if init.starts_with('[') {
                        // Multi-byte element list — decode elements (LE) into
                        // the table's byte blob. See parse_array_elements.
                        parse_array_elements(init, elem)
                    } else {
                        panic!("SPIKE LIMIT: array global initializer {init:?}");
                    };
                    (elem, size, bytes)
                }
            } else if rest.starts_with('{') {
                // Literal struct global: "{ i8, i8, i16 } { i8 65, i8 0, i16 4660 }"
                // — clang expands named structs to literal types (explicit
                // padding) whenever a global carries an initializer.
                let close = brace_inner(rest).expect("literal struct type must be `{ ... }`").len() + 1;
                let ty_str = &rest[..close + 1];
                let (size, _) = literal_ty_size_align(ty_str, &types);
                // The value is everything after the type's closing `}` —
                // do NOT split on `,` (a brace-list initializer contains
                // top-level commas; the scalar named-struct branch above
                // can split because its init is `zeroinitializer, align 2`).
                let init = rest[close + 1..].trim();
                let bytes = if init.starts_with("zeroinitializer") {
                    vec![0u8; size as usize]
                } else {
                    decode_typed_value(ty_str, init, &types)
                };
                (Ty::I8, size, bytes)
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
            // The ISR hook is clang's `msp430_intrcc` calling-convention
            // token in the return position (`define ... msp430_intrcc void
            // @isr()`); the ret type stays void and `Func.isr` is set. It is
            // not in strip_attrs' skip list, so it survives into `head`.
            let head = strip_attrs(&line[..at]);
            let isr = head.split_whitespace().any(|t| t == "msp430_intrcc");
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
            funcs.push(Func { name, ret, params, blocks, isr });
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
            let val = parse_val_typed(it.next().unwrap(), Some(ty));
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
                // Len: `i16 N` (const, unrolled — the M7 form, bounded to
                // 255 bytes) or `i16 %r` (runtime length, issue #4 — the
                // counted loop; the value is SSA-dead after the copy, so
                // isel may decrement the length slot in place).
                let len_tok = a[2].split_whitespace().last().unwrap();
                let len = if let Some(r) = len_tok.strip_prefix('%') {
                    MemLen::Reg(Val::Reg(r.to_string()))
                } else {
                    let n: u8 = len_tok.parse().expect("irparse: memcpy const len must be a u8 <= 255");
                    MemLen::Const(n)
                };
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
                out.push(Inst::Ret(Some((ty, parse_val_typed(it.next().unwrap(), Some(ty))))));
            }
        }
        "phi" => {
            let body = rest["phi".len()..].trim();
            let ty = ty_of(body.split_whitespace().next().unwrap());
            let mut incoming = Vec::new();
            for part in body.split('[').skip(1) {
                let inner = part.split(']').next().unwrap();
                let mut it = inner.split(',');
                let v = parse_val_typed(it.next().unwrap(), Some(ty));
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
            let val = parse_val_typed(it.next().unwrap(), Some(from));
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
            let a = parse_val_typed(it.next().unwrap(), Some(ty));
            let b = parse_val_typed(it.next().unwrap(), Some(ty));
            out.push(Inst::Icmp(Icmp { dst: dst.unwrap(), pred, ty, a, b }));
        }
        "select" => {
            let body = rest["select".len()..].trim();
            let parts = split_top_level(body, ',');
            let cond = parse_val(parts[0].split_whitespace().nth(1).unwrap());
            let mut it1 = parts[1].split_whitespace();
            let ty = ty_of(it1.next().unwrap());
            let a = parse_val_typed(it1.next().unwrap(), Some(ty));
            let b = parse_val_typed(parts[2].split_whitespace().nth(1).unwrap(), Some(ty));
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
            let val = parse_val_typed(it.next().unwrap(), Some(ty));
            out.push(Inst::Freeze(ir::Freeze { dst: dst.unwrap(), ty, val }));
        }
        "fadd" | "fsub" | "fmul" | "fdiv" => {
            let body = strip_attrs(&rest[op.len()..]);
            let mut it = body.split_whitespace();
            let ty = ty_of(it.next().unwrap());
            assert!(ty == Ty::F32, "irparse: float binop {op} must be f32, got {ty:?}");
            let a = parse_val_typed(it.next().unwrap(), Some(ty));
            let b = parse_val_typed(it.next().unwrap(), Some(ty));
            let o = match op.as_str() {
                "fadd" => FBinOp::FAdd,
                "fsub" => FBinOp::FSub,
                "fmul" => FBinOp::FMul,
                _ => FBinOp::FDiv,
            };
            out.push(Inst::FloatBin(FloatBin { dst: dst.unwrap(), op: o, a, b }));
        }
        "fcmp" => {
            let body = strip_attrs(&rest["fcmp".len()..]);
            let mut it = body.split_whitespace();
            let pred = it.next().unwrap().to_string();
            const FPREDS: [&str; 16] = [
                "false", "oeq", "ogt", "oge", "olt", "ole", "one", "ord",
                "ueq", "ugt", "uge", "ult", "ule", "une", "uno", "true",
            ];
            if !FPREDS.contains(&pred.as_str()) {
                panic!("SPIKE: unsupported fcmp predicate {pred:?} in line: {line}");
            }
            let ty = ty_of(it.next().unwrap());
            assert!(ty == Ty::F32, "irparse: fcmp must be f32, got {ty:?}");
            let a = parse_val_typed(it.next().unwrap(), Some(ty));
            let b = parse_val_typed(it.next().unwrap(), Some(ty));
            out.push(Inst::Fcmp(Fcmp { dst: dst.unwrap(), pred, a, b }));
        }
        "fptosi" | "fptoui" | "sitofp" | "uitofp" | "fpext" | "fptrunc" => {
            let body = strip_attrs(&rest[op.len()..]);
            let to_i = body.rfind(" to ").unwrap_or_else(|| panic!("irparse: malformed {op} (missing 'to') in line: {line}"));
            let (lhs, rhs) = (body[..to_i].trim(), body[to_i + 4..].trim());
            let mut it = lhs.split_whitespace();
            let from = ty_of(it.next().unwrap());
            let val = parse_val_typed(it.next().unwrap(), Some(from));
            let to = ty_of(rhs);
            let o = match op.as_str() {
                "fptosi" => FloatConvOp::FpToSi,
                "fptoui" => FloatConvOp::FpToUi,
                "sitofp" => FloatConvOp::SiToFp,
                "uitofp" => FloatConvOp::UiToFp,
                "fpext" => FloatConvOp::Fpext,
                _ => FloatConvOp::Fptrunc,
            };
            out.push(Inst::FloatConv(FloatConv { dst: dst.unwrap(), op: o, from, val, to }));
        }
        other => panic!("SPIKE LIMIT: unsupported opcode {other:?} in line: {line}"),
    }
    out
}
