# Const Structs in Flash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support `constant %struct.S` globals (and arrays of them) by decoding clang's literal struct initializers into the flat byte blob the existing RETLW const-table machinery already consumes.

**Architecture:** Three scoped changes, all in the `-O1` driver path (the advisory's scope):
1. `irparse`: decode clang's literal struct initializers (`{ i8, i8, i16 } { i8 65, i8 0, i16 4660 }`) into flat little-endian bytes with the same alignment layout the type table computes.
2. `irparse` `fold_gep`: support the array-of-struct element GEP `[N x %struct.S], ptr @g, i16 0, i16 %i` (the index after an array-of-struct descent strides by `sizeof(%struct.S)`), which clang -O1 emits for `&CARR[i]`.
3. `irparse` `parse_call_arg`: preserve `byval(...)`/`sret(...)` attrs on the inlined-GEP branch (clang emits `ptr ... byval(%struct.S) align 2 getelementptr ...` for by-value struct-element args).

`isel` needs no changes: the const read paths (`emit_ptr_load_byte` RETLW readers, byval byte-copy loop, constant-length memcpy source) already operate on the flat `Global.bytes` blob and accept any `(k, terms)` index.

Deferred (out of scope): O0 struct-FIELD GEP descent (`%struct.S, ptr %p, i32 0, i32 1`) — clang -O1 folds field offsets into i8-offset paren GEPs that are already handled. Such GEPs keep panicking loudly.

**Tech Stack:** Rust workspace (crates: irparse, driver, alloc, isel, pic14_sim). clang 20.1.8 (pinned in flake) as the IR producer; `pic14-sim` as the execution oracle.

**Spec:** https://github.com/apojomovsky/epic-cc/issues/5

## Global Constraints

- Workspace crate test gate: `nix develop --command bash scripts/ci-test.sh` (runs `cargo test -p <crate>` per crate; flake sets `PIC8_CLANG_UNWRAPPED`, `PIC8_CLANG_RESOURCE_DIR`, `PIC8_GPASM`).
- `irparse` contract (crate doc): any structurally malformed input panics loudly rather than silently misparsing. New decode paths must panic loudly on shapes clang does not emit.
- All struct sizes and field offsets must stay ≤ 255 (byte-addressed RAM; existing asserts in `compute_struct`).
- Const tables may span up to 65535 bytes; RAM globals are byte-addressed and stay ≤ 255 (existing asserts, keep them).
- Little-endian scalar layout; field offsets via `round_up(off, falign)`; struct size = `round_up(total, max_align)` — identical to `compute_struct` (`crates/irparse/src/lib.rs:345-358`).
- `clang -target msp430 -O1` is the only IR producer. Verified probe shapes (clang 20.1.8, all reproduced in `/tmp/conststruct/probe*.ll`):
  - `@C1 = constant { i8, i8, i16 } { i8 65, i8 0, i16 4660 }, align 2` — literal type with explicit padding.
  - `@C2 = constant { { i8, i8, i16 }, i8, i8 } { { i8, i8, i16 } { i8 66, i8 0, i16 22136 }, i8 67, i8 0 }, align 2` — nested.
  - `@CA = constant { [3 x i8], i8, i16 } { [3 x i8] c"abc", i8 0, i16 4951 }, align 2` — array field.
  - `@CF = constant { float, i8, i8 } { float 1.500000e+00, i8 81, i8 0 }, align 2` — float field.
  - `@CARR = constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2` — array of literal structs.
  - `@g = global { i8, i8, i16 } { i8 65, i8 0, i16 4660 }, align 2` — RAM globals with initializers also use literal types.
  - `getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16 %0` — array-of-struct element GEP (variable index); clang's own codegen proves stride = `sizeof(%struct.Pair)` = 4 (`add r12,r12; add r12,r12`).
  - `call void @take_byval(ptr noundef nonnull byval(%struct.Pair) align 2 getelementptr inbounds nuw (i8, ptr @CARR, i16 4))` — inlined-GEP byval arg.
  - `call void @take_byval(ptr noundef nonnull byval(%struct.Pair) align 2 @C1)` — plain-global byval (already handled by the non-GEP branch).
  - Named struct decl `%struct.Pair = type { i8, i16 }` always accompanies the GEP forms.

---

### Task 1: Decode literal struct initializers into flat bytes

**Files:**
- Modify: `crates/irparse/src/lib.rs` — `ty_size_align` (~line 296), new helpers next to `parse_array_elements` (~line 150), global-parse array/struct branches (~lines 739-785)
- Test: `crates/irparse/tests/parse_ll.rs`

**Interfaces:**
- Consumes: existing `StructTypes` (`HashMap<String, StructInfo>`), `brace_inner`, `split_top_level`, `matching_bracket`, `parse_string_literal`, `parse_array_elements`, `parse_val_typed`, `round_up`, `ty_size_align`.
- Produces: `fn literal_ty_size_align(t: &str, types: &StructTypes) -> (u16, u8)` — size/alignment of a literal `{ ... }` struct type string (recurses into nested literals, arrays, named refs).
- Produces: `fn decode_typed_value(ty: &str, value: &str, types: &StructTypes) -> Vec<u8>` — one constant of any supported type into its flat LE byte blob (`zeroinitializer`, scalar, `c"..."`/element-list array, literal struct).
- Produces: `fn decode_literal_struct(ty: &str, init: &str, types: &StructTypes) -> Vec<u8>` — a literal struct initializer into the blob, fields placed at their aligned offsets.
- Produces: `ty_size_align` gains a `{` arm delegating to `literal_ty_size_align`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/irparse/tests/parse_ll.rs` (after the existing struct tests, ~line 554):

```rust
// Issue #5: clang -O1 prints const struct globals with EXPANDED literal
// types (explicit padding) — `{ i8, i8, i16 }` for `struct { char; short }`.
// The decode must flatten the initializer into the table's byte blob using
// the same alignment layout as the type table.
const CONST_STRUCTS: &str = r#"
%struct.Pair = type { i8, i16 }
@C1 = dso_local constant { i8, i8, i16 } { i8 65, i8 0, i16 4660 }, align 2
@C2 = dso_local constant { { i8, i8, i16 }, i8, i8 } { { i8, i8, i16 } { i8 66, i8 0, i16 22136 }, i8 67, i8 0 }, align 2
@CA = dso_local constant { [3 x i8], i8, i16 } { [3 x i8] c"abc", i8 0, i16 4951 }, align 2
@CF = dso_local constant { float, i8, i8 } { float 1.500000e+00, i8 81, i8 0 }, align 2
@CARR = dso_local constant [2 x { i8, i8, i16 }] [{ i8, i8, i16 } { i8 68, i8 0, i16 4369 }, { i8, i8, i16 } { i8 69, i8 0, i16 8738 }], align 2
@CZ = dso_local constant { i8, i8, i16 } zeroinitializer, align 2
@gr = dso_local global { i8, i8, i16 } { i8 71, i8 0, i16 0x0102 }, align 2
define dso_local void @main() {
  ret void
}
"#;

#[test]
fn decodes_literal_struct_initializers_to_flat_bytes() {
    let m = parse_ll(CONST_STRUCTS);
    let g = |n: &str| m.globals.iter().find(|g| g.name == n).unwrap();

    // C1 = { 'A', pad 0, 0x1234 } -> [0x41, 0x00, 0x34, 0x12]
    let c1 = g("C1");
    assert!(c1.is_const);
    assert_eq!(c1.size, 4);
    assert_eq!(c1.bytes, vec![0x41, 0x00, 0x34, 0x12]);

    // C2 = { { 'B', pad, 0x5678 }, 'C', pad } -> size 6
    let c2 = g("C2");
    assert_eq!(c2.size, 6);
    assert_eq!(c2.bytes, vec![0x42, 0x00, 0x78, 0x56, 0x43, 0x00]);

    // CA = { "abc", pad, 0x1357 } -> size 6
    let ca = g("CA");
    assert_eq!(ca.size, 6);
    assert_eq!(ca.bytes, vec![0x61, 0x62, 0x63, 0x00, 0x57, 0x13]);

    // CF = { 1.5f (0x3FC00000 LE), 'Q', pad } -> size 6
    let cf = g("CF");
    assert_eq!(cf.size, 6);
    assert_eq!(cf.bytes, vec![0x00, 0x00, 0xC0, 0x3F, 0x51, 0x00]);

    // CARR = two { i8, i8, i16 } elements -> size 8, concatenated
    let carr = g("CARR");
    assert_eq!(carr.size, 8);
    assert_eq!(carr.bytes, vec![0x44, 0x00, 0x11, 0x11, 0x45, 0x00, 0x22, 0x22]);

    // zeroinitializer literal struct -> zeros of the layout size
    let cz = g("CZ");
    assert_eq!(cz.size, 4);
    assert_eq!(cz.bytes, vec![0u8; 4]);

    // RAM struct with an initializer keeps the same decode (and is not const)
    let gr = g("gr");
    assert!(!gr.is_const);
    assert_eq!(gr.size, 4);
    assert_eq!(gr.bytes, vec![0x47, 0x00, 0x02, 0x01]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop --command cargo test -p irparse decodes_literal_struct_initializers_to_flat_bytes`
Expected: FAIL — `panic: SPIKE LIMIT: struct global initializer "{ i8 65, ... }"` (the parse hits the existing panic at `crates/irparse/src/lib.rs:779`).

- [ ] **Step 3: Implement the decoder**

In `crates/irparse/src/lib.rs`:

(a) Add the `{` arm to `ty_size_align` (the match in `ty_size_align`, after the `%` arm, ~line 308):

```rust
    } else if t.starts_with('{') {
        // Literal struct type (`{ i8, i8, i16 }`): layout by the same
        // rules as `compute_struct`. Literal structs are value types
        // (no cycles), so plain recursion terminates.
        literal_ty_size_align(t, types)
    } else {
```

(b) Add the three helpers immediately after `parse_array_elements` (~line 150):

```rust
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
    let ty = ty.trim();
    let value = value.trim();
    if value.starts_with('{') {
        // `{ i8, i8, i16 } { i8 66, ... }` — the first brace group is the
        // self-type; `brace_inner` stops at its closing brace.
        let inner = brace_inner(value).unwrap_or("");
        let rest = value[inner.len() + 2..].trim();
        return if rest.starts_with('{') { rest } else { value };
    }
    if ty.starts_with('[') && value.starts_with('[') {
        // `[3 x i8] c"abc"` / `[2 x T] [ ... ]` — first bracket group is
        // the self-type.
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
                    out.extend(decode_typed_value(elem, strip_self_type(elem, elt), types));
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
            "SPIKE LIMIT: array value {value:?} decodes to {} bytes, expected {expect} for {ty:?}"
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
    let ty_fields: Vec<&str> = split_top_level(inner, ',').map(|s| s.trim()).collect();
    let (size, _) = literal_ty_size_align(ty, types);
    let mut blob = vec![0u8; size as usize];
    if init.starts_with("zeroinitializer") {
        return blob;
    }
    let v_inner = brace_inner(init)
        .unwrap_or_else(|| panic!("SPIKE LIMIT: struct initializer {init:?} for type {ty:?}"));
    let values: Vec<&str> = split_top_level(v_inner, ',').map(|s| s.trim()).collect();
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
```

(c) Wire the global-parse branches (`parse_ll` main loop, ~lines 739-785). In the array branch, before `let elem = ty_of(...)`:

```rust
                let elem_str = pit.next().unwrap();
                if elem_str.starts_with('{') {
                    // "[N x { ... }] { {...}, {...} }" — array of literal
                    // structs (clang's expanded form for struct arrays).
                    let (es, _) = literal_ty_size_align(elem_str, types);
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
                        decode_typed_value(&rest[..close + 1], init, types)
                    };
                    (Ty::I8, size, bytes)
                } else {
                    let elem = ty_of(elem_str);
                    // ... existing `let size = n * elem.bytes() ...` block
                }
```

And a new branch between the array branch and the `%` branch (~line 770), for literal-struct globals:

```rust
            } else if rest.starts_with('{') {
                // Literal struct global: "{ i8, i8, i16 } { i8 65, i8 0, i16 4660 }"
                // — clang expands named structs to literal types (explicit
                // padding) whenever a global carries an initializer.
                let close = brace_inner(rest).expect("literal struct type must be `{ ... }`").len() + 1;
                let ty_str = &rest[..close + 1];
                let (size, _) = literal_ty_size_align(ty_str, types);
                let init = rest[close + 1..].trim().split(',').next().unwrap().trim();
                let bytes = if init.starts_with("zeroinitializer") {
                    vec![0u8; size as usize]
                } else {
                    decode_typed_value(ty_str, init, types)
                };
                (Ty::I8, size, bytes)
```

Note: the `%struct.X` named branch above it stays unchanged (zeroinitializer only; clang never prints a named-type global with a literal initializer — probes confirm literal expansion).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop --command cargo test -p irparse decodes_literal_struct_initializers_to_flat_bytes`
Expected: PASS.

- [ ] **Step 5: Run the full irparse suite**

Run: `nix develop --command cargo test -p irparse`
Expected: all existing tests still PASS (the new `ty_size_align` `{` arm and the array-branch restructure must not disturb them).

- [ ] **Step 6: Commit**

```bash
git add crates/irparse/src/lib.rs crates/irparse/tests/parse_ll.rs
git commit -m "feat(irparse): decode literal struct initializers into flat bytes (issue #5)"
```

---

### Task 2: Array-of-struct element GEP stride in fold_gep

**Files:**
- Modify: `crates/irparse/src/lib.rs` — `fold_gep` (~line 520)
- Test: `crates/irparse/tests/parse_ll.rs`

**Interfaces:**
- Consumes: `fold_gep(source_ty, index_parts, types) -> (u8, Vec<(u8, String)>)` signature unchanged; `ty_size_align` (now handles `{` too).
- Produces: GEPs of shape `[N x %struct.S], ptr @g, i16 0, i16 %i` fold to `(k, [(sizeof(S), "i")])` — the index after an array-of-struct descent is the ELEMENT selector (stride = struct size), matching clang's codegen. Struct sources (`%struct.S, ...`) and field selects keep panicking.

- [ ] **Step 1: Write the failing tests**

Add to `crates/irparse/tests/parse_ll.rs` (near the existing GEP tests):

```rust
// Issue #5: clang -O1 lowers `&CARR[i]` on a const struct array to
// `getelementptr [2 x %struct.Pair], ptr @CARR, i16 0, i16 %i` — the index
// after an array-of-struct descent is the ELEMENT selector, striding by
// sizeof(%struct.Pair). Field offsets ride as separate i8-offset GEPs, so
// no further struct descent is needed.
const STRUCT_ARRAY_GEP: &str = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x %struct.Pair] [%struct.Pair { i8 68, i16 4369 }, %struct.Pair { i8 69, i16 8738 }], align 2
define dso_local void @main(i16 %i) {
  %p = getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16 %i
  ret void
}
"#;

#[test]
fn folds_struct_array_element_gep_to_struct_stride() {
    let m = parse_ll(STRUCT_ARRAY_GEP);
    let body = &m.funcs[0].blocks[0].insts;
    match &body[0] {
        Inst::Gep(g) => {
            assert_eq!(g.base, GepBase::Global("CARR".to_string()));
            assert_eq!(g.k, 0);
            assert_eq!(g.terms, vec![(4, "i".to_string())], "element stride = sizeof(%struct.Pair) = 4");
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}

#[test]
fn folds_struct_array_constant_element_gep_to_byte_offset() {
    let ll = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x %struct.Pair] [%struct.Pair { i8 68, i16 4369 }, %struct.Pair { i8 69, i16 8738 }], align 2
define dso_local void @main() {
  %p = getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16 1
  ret void
}
"#;
    let m = parse_ll(ll);
    match &m.funcs[0].blocks[0].insts[0] {
        Inst::Gep(g) => {
            assert_eq!(g.k, 4, "constant element 1 -> byte offset 4");
            assert!(g.terms.is_empty());
        }
        other => panic!("expected Gep, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop --command cargo test -p irparse folds_struct_array`
Expected: FAIL — `panic: irparse: gep on struct-typed source %struct.Pair unsupported (struct descent not implemented)`.

- [ ] **Step 3: Implement the element-step in fold_gep**

Replace the struct check in `fold_gep` (`crates/irparse/src/lib.rs:526-532`) and track the previous level:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop --command cargo test -p irparse folds_struct_array`
Expected: PASS (both tests).

- [ ] **Step 5: Run the full irparse suite** — the existing `struct_gep_source_panics` test must still panic:

Run: `nix develop --command cargo test -p irparse`
Expected: all PASS, including `struct_gep_source_panics` (`%struct.S, ptr @s, i16 0, i16 1` — first index on a struct source, `from_array = false` → panic).

- [ ] **Step 6: Commit**

```bash
git add crates/irparse/src/lib.rs crates/irparse/tests/parse_ll.rs
git commit -m "feat(irparse): fold array-of-struct element GEPs by struct stride (issue #5)"
```

---

### Task 3: Preserve byval/sret attrs on inlined-GEP call args

**Files:**
- Modify: `crates/irparse/src/lib.rs` — `parse_call_arg` (~line 593)
- Test: `crates/irparse/tests/parse_ll.rs`

**Interfaces:**
- Consumes: `tokenize_parens`, `types` (`StructTypes`).
- Produces: `CallArg` with `byval: Option<u8>` / `sret: bool` set even when the arg carries an inlined GEP (clang's shape for by-value struct-element args: `ptr ... byval(%struct.S) align 2 getelementptr ...`).

- [ ] **Step 1: Write the failing test**

Add to `crates/irparse/tests/parse_ll.rs` (near the byval/SRET tests, ~line 640):

```rust
// Issue #5: by-value struct-element args carry BOTH the byval attr and an
// inlined GEP (`ptr ... byval(%struct.S) align 2 getelementptr ...`). The
// inlined-GEP branch must preserve the attr or isel's byval copy is
// skipped and the callee ABI silently breaks.
const BYVAL_GEP_ARG: &str = r#"
%struct.Pair = type { i8, i16 }
@CARR = dso_local constant [2 x %struct.Pair] [%struct.Pair { i8 68, i16 4369 }, %struct.Pair { i8 69, i16 8738 }], align 2
define dso_local void @take_byval(ptr nocapture noundef readonly byval(%struct.Pair) align 2 %0) local_unnamed_addr #0 {
  ret void
}
define dso_local void @main() local_unnamed_addr #1 {
  tail call void @take_byval(ptr noundef nonnull byval(%struct.Pair) align 2 getelementptr inbounds nuw (i8, ptr @CARR, i16 4))
  ret void
}
"#;

#[test]
fn preserves_byval_on_inlined_gep_call_arg() {
    let m = parse_ll(BYVAL_GEP_ARG);
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    match &main.blocks[0].insts[0] {
        Inst::Call(c) => {
            assert_eq!(c.func, "take_byval");
            assert_eq!(c.args.len(), 1);
            let arg = &c.args[0];
            assert_eq!(arg.byval, Some(4), "byval(%struct.Pair) -> size 4");
            assert!(!arg.sret);
            assert!(matches!(arg.val, Val::Reg(_)), "inlined GEP is synthesized into a reg: {:?}", arg.val);
        }
        other => panic!("expected Call, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop --command cargo test -p irparse preserves_byval_on_inlined_gep_call_arg`
Expected: FAIL — `assertion failed: arg.byval == Some(4)` (currently `None`).

- [ ] **Step 3: Implement the attr scan**

In `parse_call_arg` (`crates/irparse/src/lib.rs:595-610`), replace the inlined-GEP branch's `CallArg { ty, val: Val::Reg(n), byval: None, sret: false }` with:

```rust
        // The attr prefix before the inlined GEP can carry byval/sret
        // (`ptr ... byval(%struct.S) align 2 getelementptr ...` — clang's
        // shape for passing a struct element by value). Preserve them or
        // the callee ABI silently breaks.
        let mut byval = None;
        let mut sret = false;
        for t in tokenize_parens(&a[..gpos]) {
            if let Some(rest) = t.strip_prefix("byval(") {
                let inner = rest.trim_end_matches(')');
                let info = types.get(inner.trim_start_matches('%')).unwrap_or_else(|| {
                    panic!("irparse: unknown byval type {inner}")
                });
                byval = Some(info.size);
            } else if t.starts_with("sret(") {
                sret = true;
            }
        }
        CallArg { ty, val: Val::Reg(n), byval, sret }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `nix develop --command cargo test -p irparse preserves_byval_on_inlined_gep_call_arg`
Expected: PASS.

- [ ] **Step 5: Run the full irparse suite**

Run: `nix develop --command cargo test -p irparse`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/irparse/src/lib.rs crates/irparse/tests/parse_ll.rs
git commit -m "feat(irparse): preserve byval/sret on inlined-GEP call args (issue #5)"
```

---

### Task 4: End-to-end const struct fixture through the simulator

**Files:**
- Create: `crates/driver/tests/fixtures/const_struct.c`
- Create: `crates/driver/tests/const_struct_e2e.rs`
- Test: `crates/driver/tests/const_struct_e2e.rs`

**Interfaces:**
- Consumes: the `const_table_e2e.rs` harness pattern (clang → irparse → wholeprog → legalize → callgraph → alloc → isel → banking → peephole for the layout; `CARGO_BIN_EXE_driver` for the HEX; `pic14_sim` for execution).
- Produces: an acceptance test proving const structs decode, array-of-struct GEPs fold, byval copies read the RETLW table, and constant-length memcpy reads flash — all at runtime with hand-computed RAM results.

- [ ] **Step 1: Write the failing fixture and test**

`crates/driver/tests/fixtures/const_struct.c`:

```c
// Issue #5: const (flash) structs. Exercises the flat-byte decode of
// clang's literal struct initializers, array-of-struct element GEPs
// (variable index), byval copies of const structs (plain global and
// inlined-GEP element forms), and runtime byte-indexed flash reads of a
// const struct (the RETLW-table path). All reads land in volatile
// globals read back by the sim.
//
// NOTE: no constant-length `__builtin_memcpy(buf, &C1, 4)` — clang -O1
// folds a memcpy from a known const into a constant store, so no flash
// read reaches the IR. The `(const unsigned char *)&C1` byte reads keep
// the index runtime (`idx`) so the RETLW readers actually run.
struct Pair { char a; short b; };

const struct Pair C1 = { 'A', 0x1234 };
const struct Pair CARR[2] = { { 'D', 0x1111 }, { 'E', 0x2222 } };

volatile unsigned char idx;
volatile unsigned char out_a, out_a2, out_a3, out_m0, out_m1;
volatile unsigned short out_b, out_b2, out_b3;

__attribute__((noinline)) void byval_c1(struct Pair p) {
    out_a = p.a;
    out_b = p.b;
}
__attribute__((noinline)) void byval_elem1(struct Pair p) {
    out_a2 = p.a;
    out_b2 = p.b;
}
__attribute__((noinline)) void byval_var(struct Pair p) {
    out_a3 = p.a;
    out_b3 = p.b;
}

void main(void) {
    byval_c1(C1);            // byval of a const struct (plain global)
    byval_elem1(CARR[1]);    // byval of a const struct element (inlined GEP)
    byval_var(CARR[idx]);    // byval with a variable element index
    out_m0 = ((const unsigned char *)&C1)[idx];      // flash byte read
    out_m1 = ((const unsigned char *)&C1)[idx + 2];  // flash byte read, +2
}
```

`crates/driver/tests/const_struct_e2e.rs` (mirrors `const_table_e2e.rs`):

```rust
//! Issue #5 acceptance: const struct globals decode into flat flash bytes
//! and read correctly through the RETLW table readers at runtime.
//!
//! Hand-computed expectations (sim sets `idx` = 1 before run):
//!   - C1 = { 'A', pad, 0x1234 } -> flash [0x41, 0x00, 0x34, 0x12]
//!   - CARR[0] = { 'D', 0x1111 }, CARR[1] = { 'E', 0x2222 }
//!   - byval_c1(C1)         -> out_a = 0x41, out_b = 0x1234
//!   - byval_elem1(CARR[1]) -> out_a2 = 0x45, out_b2 = 0x2222
//!   - byval_var(CARR[idx]) -> out_a3 = 0x45, out_b3 = 0x2222 (idx=1)
//!   - ((u8*)&C1)[idx]      -> out_m0 = 0x41 (byte 1)
//!   - ((u8*)&C1)[idx + 2]  -> out_m1 = 0x34 (byte 3)

use std::collections::HashMap;
use std::process::Command;

fn const_struct_layout() -> alloc::AllocLayout {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let ll = Command::new(clang)
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
            "tests/fixtures/const_struct.c",
        ])
        .output()
        .expect("run clang");
    assert!(ll.status.success(), "clang: {}", String::from_utf8_lossy(&ll.stderr));
    let ll_text = String::from_utf8(ll.stdout).unwrap();

    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let mut addrs: HashMap<String, u16> = HashMap::new();
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = isel::select(&device::PIC16F877A, &m, &addrs);
    let asm = banking::assign_banks(&device::PIC16F877A, &asm);
    let _ = peephole::optimize(&asm);
    layout
}

#[test]
fn const_struct_reads_run_correctly() {
    let layout = const_struct_layout();
    let addr = |n: &str| *layout.globals.get(n).expect(n) as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_driver"))
        .args(["tests/fixtures/const_struct.c", "tests/fixtures/const_struct.hex"])
        .output()
        .expect("run driver");
    assert!(out.status.success(), "driver: {}", String::from_utf8_lossy(&out.stderr));

    let hex = std::fs::read_to_string("tests/fixtures/const_struct.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.ram_mut()[addr("idx")] = 1; // CARR[idx] -> element 1
    p.run(200_000);
    assert!(p.halted());

    assert_eq!(p.ram()[addr("out_a")], 0x41, "out_a = C1.a");
    assert_eq!(p.ram()[addr("out_a2")], 0x45, "out_a2 = CARR[1].a");
    assert_eq!(p.ram()[addr("out_a3")], 0x45, "out_a3 = CARR[idx].a (idx=1)");
    let b = |n: &str| {
        let a = addr(n);
        u16::from(p.ram()[a]) | (u16::from(p.ram()[a + 1]) << 8)
    };
    assert_eq!(b("out_b"), 0x1234, "out_b = C1.b");
    assert_eq!(b("out_b2"), 0x2222, "out_b2 = CARR[1].b");
    assert_eq!(b("out_b3"), 0x2222, "out_b3 = CARR[idx].b (idx=1)");
    assert_eq!(p.ram()[addr("out_m0")], 0x41, "out_m0 = ((u8*)&C1)[1]");
    assert_eq!(p.ram()[addr("out_m1")], 0x34, "out_m1 = ((u8*)&C1)[3]");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `nix develop --command cargo test -p driver --test const_struct_e2e`
Expected: FAIL — the driver panics in irparse (`SPIKE LIMIT: struct global initializer`) before producing a HEX.

- [ ] **Step 3: Verify the fixture's clang IR shapes match the plan's assumptions**

Run: `nix develop --command bash -c '$PIC8_CLANG_UNWRAPPED -target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc -resource-dir $PIC8_CLANG_RESOURCE_DIR -o - crates/driver/tests/fixtures/const_struct.c'` and grep for:
- `constant { i8, i8, i16 } { i8 65, ... }` (C1) and `constant [2 x { i8, i8, i16 }]` (CARR),
- `getelementptr inbounds nuw [2 x %struct.Pair], ptr @CARR, i16 0, i16` (variable index),
- `byval(%struct.Pair) align 2 getelementptr` and `byval(%struct.Pair) align 2 @C1`,
- `getelementptr inbounds nuw i8, ptr @C1, i16` (the byte-indexed flash reads; clang may split `idx + 2` into a chained GEP — the isel chain-folding handles it).

If a shape differs (e.g. clang folds the variable index or prints a different initializer form), adjust the fixture (e.g. make `idx` read through a volatile) and re-verify. Do not change the implementation to chase a fixture clang does not emit.

- [ ] **Step 4: Run the test to verify it passes**

Run: `nix develop --command cargo test -p driver --test const_struct_e2e`
Expected: PASS.

- [ ] **Step 5: Run the full workspace suite**

Run: `nix develop --command bash scripts/ci-test.sh`
Expected: all crates' tests PASS (this is the CI gate; it also runs the gpasm cross-checks and every other e2e).

- [ ] **Step 6: Commit**

```bash
git add crates/driver/tests/fixtures/const_struct.c crates/driver/tests/const_struct_e2e.rs
git commit -m "test(driver): e2e const struct reads through the simulator (issue #5)"
```

---

## Self-Review

**Spec coverage (issue #5 body):**
- "A constant struct global (`constant %struct.S`) panics today" → Task 1 decodes clang's literal-struct initializer form (the only form clang -O1 emits) into the flat blob; the panic is gone for real clang output. ✅
- "Decide on a representation for constant structs, likely a flat byte blob plus the existing RETLW reader" → adopted: `Global.bytes` flat blob, unchanged RETLW machinery. ✅
- "make sure the byval and sret paths treat them correctly" → Task 3 preserves byval on inlined-GEP args (the missing ABI path); plain-global byval and runtime byte-indexed flash reads (the RETLW path) are covered by Task 4; sret attrs are preserved so the existing sret handling applies rather than silently mis-ABI'ing. ✅
- "arrays of bytes only" → Task 1 also covers arrays of structs (`[N x { ... }]` globals + `[N x %struct.S]` GEPs), which const struct arrays require. ✅

**Placeholder scan:** every step carries concrete code or a concrete command; no TBD/TODO/"handle edge cases". ✅

**Type consistency:** `literal_ty_size_align` / `decode_typed_value` / `decode_literal_struct` signatures are defined in Task 1 and used identically in Tasks 1 and 4's fixture path; `fold_gep` keeps its existing signature; `CallArg { ty, val, byval, sret }` fields match `ir::CallArg`. ✅

**Deferred, documented:** O0 struct-field GEP descent (`%struct.S, ptr %p, i32 0, i32 1`) and named-struct globals with literal initializers (clang emits neither in the -O1 driver path) — both keep loud panics, per the advisory's scope.
