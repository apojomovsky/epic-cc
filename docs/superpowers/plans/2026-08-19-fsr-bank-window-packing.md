# FSR Bank-Window Global Packing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix issue #7 ("Place FSR-accessed globals inside single bank windows") by making `alloc`'s global-placement algorithm bin-pack across the device's GPR bank windows when today's simple sequential placement would otherwise fail — so a program with several differently-sized globals doesn't spuriously exceed device RAM capacity when a better arrangement would fit.

**Architecture:** `alloc`'s existing global-placement loop uses one monotonically-advancing cursor: each global is placed at the current cursor, and any global that doesn't fit the current bank pushes the cursor to the *next* bank, permanently abandoning whatever free space is left in every bank behind it. For a realistic mix of large and small globals, that abandoned space can push total demand over the device's real (aggregate) capacity even when a different placement order would fit everything. The fix is a two-phase strategy: try today's sequential algorithm first (unchanged — every currently-succeeding program keeps byte-for-byte identical addresses); only if it fails, fall back to a First-Fit-Decreasing bin-packing pass that tracks each bank's free space *independently* (so a small global declared late can still land in an early bank's leftover space) and places globals largest-first (so large objects claim whole banks before small objects fragment them). Only panic — with a message that explains the real constraint — when neither arrangement fits.

**Tech Stack:** Rust, Cargo workspace, `nix develop` dev shell.

**Spec:** GitHub issue #7 ("Place FSR-accessed globals inside single bank windows") — the underlying hardware constraint it documents (FSR+IRP indirect access requires the whole accessed object to fit inside one of the device's four GPR bank windows) is enforced today by a loud panic in `crates/isel/src/lib.rs`'s `fsr_window` function; that panic and its accompanying doc comment (`crates/isel/src/lib.rs:128-149`) are the ground truth for what a "bank window" is and stay untouched by this plan — this plan only changes how `alloc` *places* globals so that constraint is easier to satisfy.

## Scope boundary

**In scope:** global placement only (`alloc::allocate`'s globals loop). This matches issue #7's own title and body, which both say "globals" specifically.

**Out of scope, deliberately:** local/frame placement (`place_contiguous`, the overlay/call-graph frame-base machinery). Locals already get *some* protection from `place_contiguous`'s existing per-value bank-stepping, and reordering locals within a frame would require threading the same reordering through `frame_end`'s overlay-base derivation (used by callee frame placement and the ISR-disjoint-base computation) — a materially bigger, riskier change than issue #7 actually asks for. If locals need the same treatment later, that's a natural, separately-scoped follow-up.

## Global Constraints

- **Every program that successfully allocates today must keep byte-for-byte identical global addresses after this change.** The fallback only activates when today's sequential algorithm would otherwise panic — this is what makes the change safe to land without an exhaustive audit of every existing address-asserting test.
- **`isel`'s `fsr_window` panic (`crates/isel/src/lib.rs:137-152`) is not touched.** It remains the last-resort defense against a genuinely malformed placement; this plan just makes `alloc` far less likely to ever produce one.
- Conventional commits, single line, at most 3 lines. Branch: `fix/fsr-bank-window-placement` (already created, forked from fresh master).

---

## File Structure

- Modify: `crates/alloc/src/lib.rs` — add `try_place_at`, `try_place_globals_sequential`, `BankCursor`, `place_globals_bin_packed`; reimplement `place_at` in terms of `try_place_at`; change `allocate()`'s globals loop to try sequential first, fall back to bin-packed, and panic with a clearer message only if both fail.
- Modify: `crates/alloc/tests/alloc.rs` — one new integration test proving the reproduction scenario (fails today, succeeds after) via the public `allocate()` entry point.

---

### Task 1: Option-returning core for sequential global placement (pure refactor)

**Files:**
- Modify: `crates/alloc/src/lib.rs`

**Interfaces:**
- Produces: `fn try_place_at(device: &Device, addr: u16, width: u8) -> Option<u16>` — same placement rule `place_at` already has (step through `device.region_for`-derived regions, respecting `align = width.min(2)`), but returns `None` instead of panicking when no region has room past the device's last bank.
- Produces: `fn try_place_globals_sequential(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>>` — walks `globals` in the given (caller-supplied) order with one monotonically-advancing cursor, exactly like `allocate()`'s current inline globals loop, returning `None` the first time any global doesn't fit (instead of panicking).
- Consumes: `Device::region_for(&self, addr: u16) -> Option<(u16, u16)>` (already public, unchanged).
- `place_at`'s existing panicking behavior and message must be preserved exactly (it becomes a thin wrapper over `try_place_at`), since `place_contiguous` (locals, out of scope) and any other existing caller must see zero behavior change.

- [ ] **Step 1: Write the failing test**

Add near the bottom of `crates/alloc/src/lib.rs`, in a new `#[cfg(test)] mod tests` block (the file has none yet — this is the first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use device::PIC16F877A;

    fn global(name: &str, size: u16) -> ir::Global {
        ir::Global { name: name.to_string(), ty: ir::Ty::I8, is_const: false, size, bytes: Vec::new(), addr: None }
    }

    #[test]
    fn try_place_at_returns_none_instead_of_panicking_past_the_last_bank() {
        // PIC16F877A's last bank ends at 0x1EF; nothing at or past 0x1F0 has
        // a region, so placing even a 1-byte value there must fail cleanly.
        assert_eq!(try_place_at(&PIC16F877A, 0x1F0, 1), None);
    }

    #[test]
    fn try_place_globals_sequential_returns_none_when_a_later_global_cannot_fit_anywhere() {
        // Three 76-byte globals, one 78-byte global, then one 4-byte global
        // (310 bytes total, well under the device's 320-byte capacity) — the
        // single advancing cursor abandons a 4-byte leftover in each of the
        // first three banks it uses, then the 78-byte global leaves only 2
        // bytes in the fourth (last) bank, too little for the trailing
        // 4-byte global with nowhere left to go. See Task 3's integration
        // test for the full derivation of these exact sizes.
        let g0 = global("g0", 76);
        let g1 = global("g1", 76);
        let g2 = global("g2", 76);
        let g3 = global("g3", 78);
        let g4 = global("g4", 4);
        let refs: Vec<&ir::Global> = vec![&g0, &g1, &g2, &g3, &g4];
        assert_eq!(try_place_globals_sequential(&PIC16F877A, &refs), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop --command cargo test -p alloc try_place`
Expected: FAIL to compile — `try_place_at`/`try_place_globals_sequential` don't exist yet.

- [ ] **Step 3: Write the implementation**

Add above `place_at`:

```rust
/// The start address for a `width`-byte value placed at the next free
/// address `addr`, or `None` if no region past `addr` has room (the device's
/// last bank has been exhausted). Same placement rule as `place_at` — step
/// through regions, `align = width.min(2)` — without the panic.
fn try_place_at(device: &Device, addr: u16, width: u8) -> Option<u16> {
    let align = width.min(2);
    let mut a = addr;
    loop {
        let (start, end) = device.region_for(a)?;
        let mut base = a.max(start);
        if base % u16::from(align) != 0 {
            base += u16::from(align) - (base % u16::from(align));
        }
        if base + u16::from(width) - 1 <= end {
            return Some(base);
        }
        a = end + 1;
    }
}
```

Replace `place_at`'s body with:

```rust
fn place_at(device: &Device, addr: u16, width: u8) -> u16 {
    try_place_at(device, addr, width).unwrap_or_else(|| {
        let last_end = device.ram_banks.last().expect("a device has at least one GPR bank").1;
        panic!("alloc: GPR demand exceeds 0x{last_end:X} ({addr:#06x})")
    })
}
```

(This is the exact message `region_for`'s panic already produces today via the old `place_at` — `addr` here is the same `a` value that reached the end of the loop, so the message text is unchanged. Note: `region_for` — the private wrapper — becomes unused by `place_at` after this change; check whether `place_contiguous` still needs it (it does, untouched) before deciding whether `region_for` itself can be simplified — it can't be removed, just leave it as-is.)

Add below `try_place_at`:

```rust
/// `globals` placed in order with ONE monotonically-advancing cursor —
/// exactly `allocate()`'s original globals loop, extracted so it can be
/// tried before falling back to bin-packing. Returns `None` the first time
/// any global doesn't fit, rather than panicking.
fn try_place_globals_sequential(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>> {
    let mut out = HashMap::new();
    let mut addr: u16 = device.gpr_start();
    for g in globals {
        let width = g.size as u8;
        let start = try_place_at(device, addr, width)?;
        out.insert(g.name.clone(), start);
        addr = start + u16::from(width);
    }
    Some(out)
}
```

In `allocate()`, the existing inline globals loop reads exactly:

```rust
    let mut globals: HashMap<String, u16> = HashMap::new();
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut addr: u16 = device.gpr_start();
    for g in &m.globals {
        if g.is_const {
            const_globals.insert(g.name.clone());
            continue;
        }
        // RAM globals are byte-addressed: `place_at` takes a u8 width. Const
        // globals are skipped above, so only RAM sizes reach here — a RAM
        // array past 255 bytes is a parse-time error, but assert loudly
        // anyway (defense in depth against a hand-built Module).
        let width = g.size;
        assert!(width <= 255, "alloc: RAM global @{} too large ({width} bytes; RAM is byte-addressed, max 255)", g.name);
        let width = width as u8;
        let start = place_at(device, addr, width);
        globals.insert(g.name.clone(), start);
        addr = start + u16::from(width);
    }
```

(nothing after this block reads the `addr` variable it declares — `end_of_globals`, computed next, reads from the `globals` map by name instead — so it is safe to drop entirely.) Replace the whole block with:

```rust
    let mut const_globals: HashSet<String> = HashSet::new();
    let mut non_const: Vec<&ir::Global> = Vec::new();
    for g in &m.globals {
        if g.is_const {
            const_globals.insert(g.name.clone());
        } else {
            assert!(g.size <= 255, "alloc: RAM global @{} too large ({} bytes; RAM is byte-addressed, max 255)", g.name, g.size);
            non_const.push(g);
        }
    }
    let globals: HashMap<String, u16> = try_place_globals_sequential(device, &non_const)
        .unwrap_or_else(|| {
            let last_end = device.ram_banks.last().expect("a device has at least one GPR bank").1;
            panic!("alloc: GPR demand exceeds 0x{last_end:X}")
        });
```

This is Task 1's *only* change to `allocate()` — the fallback (`place_globals_bin_packed`) is not called yet; a program that fails sequential placement today still fails identically here (same panic family, "alloc: GPR demand exceeds..."), which is what keeps this task a pure, behavior-preserving refactor. Task 3 changes this exact `unwrap_or_else` call.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop --command cargo test -p alloc`
Expected: PASS — the 2 new unit tests, and every existing test in `crates/alloc/tests/alloc.rs` unchanged (including `frame_exceeding_all_banks_panics`, which exercises `place_contiguous`/`place_at`'s locals path, untouched by this refactor).

- [ ] **Step 5: Commit**

```bash
git add crates/alloc/src/lib.rs
git commit -m "refactor(alloc): extract an Option-returning core from global placement"
```

---

### Task 2: First-Fit-Decreasing bin-packing fallback (new, not yet wired in)

**Files:**
- Modify: `crates/alloc/src/lib.rs`

**Interfaces:**
- Consumes: `Device::ram_banks: &'static [(u16, u16)]` (already public).
- Produces: `fn place_globals_bin_packed(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>>` — sorts a copy of `globals` by descending `size` (Rust's `slice::sort_by` is stable, so equal-sized globals keep their original relative order), then places each into the *first* bank whose independently-tracked free-space cursor has room (respecting the same `align = width.min(2)` rule), returning `None` only if some global doesn't fit any bank at all.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block from Task 1:

```rust
    #[test]
    fn bin_packed_places_all_globals_with_no_overlaps_and_within_one_bank_each() {
        // Same reproduction input as Task 1's sequential-failure test: three
        // 76-byte globals, one 78-byte global, one 4-byte global (310 bytes
        // total). Bin-packing succeeds where the single advancing cursor
        // does not (see Task 3's integration test, which proves the
        // sequential-fails / bin-packed-succeeds contrast through the full
        // `allocate()` entry point).
        let g0 = global("g0", 76);
        let g1 = global("g1", 76);
        let g2 = global("g2", 76);
        let g3 = global("g3", 78);
        let g4 = global("g4", 4);
        let refs: Vec<&ir::Global> = vec![&g0, &g1, &g2, &g3, &g4];
        let placed = place_globals_bin_packed(&PIC16F877A, &refs).expect("bin-packing must succeed");
        assert_eq!(placed.len(), 5);

        // No two globals may overlap, and every placement must lie fully
        // within a single bank's inclusive range (never straddling one).
        let mut spans: Vec<(u16, u16)> = refs
            .iter()
            .map(|g| {
                let start = placed[&g.name];
                (start, start + g.size - 1)
            })
            .collect();
        for &(start, end) in &spans {
            assert!(
                PIC16F877A.ram_banks.iter().any(|&(bs, be)| start >= bs && end <= be),
                "global at 0x{start:03X}..=0x{end:03X} does not fit inside a single bank"
            );
        }
        spans.sort();
        for w in spans.windows(2) {
            assert!(w[0].1 < w[1].0, "overlapping placements: {:?} and {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn bin_packed_returns_none_when_total_demand_truly_exceeds_capacity() {
        // 5 objects of 70 bytes each = 350 bytes > the device's 320-byte
        // total GPR capacity (4 banks x 80 bytes) — no arrangement fits.
        let gs: Vec<ir::Global> = (0..5).map(|i| global(&format!("g{i}"), 70)).collect();
        let refs: Vec<&ir::Global> = gs.iter().collect();
        assert_eq!(place_globals_bin_packed(&PIC16F877A, &refs), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop --command cargo test -p alloc bin_packed`
Expected: FAIL to compile — `place_globals_bin_packed` doesn't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
/// One bank's independently-tracked free-space frontier during bin-packing.
struct BankCursor {
    end: u16,
    next_free: u16,
}

/// Places `globals` largest-first into whichever bank's free-space cursor
/// has room first (First-Fit-Decreasing), so a small global declared after
/// several large ones can still land in an earlier bank's leftover space —
/// unlike `try_place_globals_sequential`'s single monotonically-advancing
/// cursor, which abandons every bank's leftover the moment it moves on.
/// Returns `None` only if some global has no room in any bank once every
/// earlier (larger-or-equal) global has been placed.
fn place_globals_bin_packed(device: &Device, globals: &[&ir::Global]) -> Option<HashMap<String, u16>> {
    let mut cursors: Vec<BankCursor> =
        device.ram_banks.iter().map(|&(start, end)| BankCursor { end, next_free: start }).collect();
    let mut order: Vec<&&ir::Global> = globals.iter().collect();
    order.sort_by(|a, b| b.size.cmp(&a.size));

    let mut out = HashMap::new();
    for g in order {
        let width = g.size as u8;
        let align = width.min(2);
        let mut placed = None;
        for cursor in cursors.iter_mut() {
            let mut base = cursor.next_free;
            if base % u16::from(align) != 0 {
                base += u16::from(align) - (base % u16::from(align));
            }
            if base + u16::from(width) - 1 <= cursor.end {
                cursor.next_free = base + u16::from(width);
                placed = Some(base);
                break;
            }
        }
        out.insert(g.name.clone(), placed?);
    }
    Some(out)
}
```

Self-check while implementing: `order: Vec<&&ir::Global>` (double reference, since `globals: &[&ir::Global]` and `.iter()` over that yields `&&ir::Global`) — `g.size`/`g.name` still resolve correctly through the double reference via auto-deref, but confirm this compiles as written; if the compiler prefers a single-reference `Vec<&ir::Global>` instead (e.g. via `globals.to_vec()`), use that form instead — either is fine, just make sure it actually compiles rather than fighting the type checker.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop --command cargo test -p alloc`
Expected: PASS — all tests from Task 1 plus these two new ones. `place_globals_bin_packed` is not yet called from `allocate()`, so no existing test's behavior can change in this task.

- [ ] **Step 5: Commit**

```bash
git add crates/alloc/src/lib.rs
git commit -m "feat(alloc): add a first-fit-decreasing bin-packing fallback for globals"
```

---

### Task 3: Wire the fallback into `allocate()`, prove the reproduction case, improve the final panic

**Files:**
- Modify: `crates/alloc/src/lib.rs`
- Modify: `crates/alloc/tests/alloc.rs`

**Interfaces:**
- `allocate()`'s globals step becomes: `try_place_globals_sequential(device, &non_const).or_else(|| place_globals_bin_packed(device, &non_const)).unwrap_or_else(|| panic!(<improved message>))`.

- [ ] **Step 1: Write the failing test**

Add to `crates/alloc/tests/alloc.rs`:

```rust
#[test]
fn a_global_layout_sequential_placement_cannot_fit_succeeds_via_bin_packing() {
    // Three 76-byte globals, one 78-byte global, then one 4-byte global (310
    // bytes total, under the device's 320-byte capacity) — declared in an
    // order where the single sequential cursor abandons a 4-byte leftover in
    // each of the first three banks it uses, then the 78-byte global leaves
    // only 2 bytes in the fourth (last) bank — too little for the trailing
    // 4-byte global, which then has no fifth bank to step into. This is the
    // exact reproduction Task 1's and Task 2's unit tests use in isolation;
    // this test proves the fix through the full public `allocate()` entry
    // point.
    let mut src = String::new();
    for i in 0..5 {
        src.push_str(&format!("global g{i} i8\n"));
    }
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    let sizes = [76u16, 76, 76, 78, 4];
    for i in 0..5 {
        m.globals[i].size = sizes[i];
    }

    // Before this plan, this call panics ("alloc: GPR demand exceeds
    // 0x1EF..."). After Task 3, it must succeed, and every global must be
    // placed within exactly one bank with no two overlapping.
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.globals.len(), 5);
    let mut spans: Vec<(u16, u16)> = (0..5)
        .map(|i| {
            let name = format!("g{i}");
            let start = out.globals[&name];
            (start, start + sizes[i] - 1)
        })
        .collect();
    for &(start, end) in &spans {
        assert!(
            PIC16F877A.ram_banks.iter().any(|&(bs, be)| start >= bs && end <= be),
            "global at 0x{start:03X}..=0x{end:03X} does not fit inside a single bank"
        );
    }
    spans.sort();
    for w in spans.windows(2) {
        assert!(w[0].1 < w[1].0, "overlapping placements: {:?} and {:?}", w[0], w[1]);
    }
}

#[test]
#[should_panic(expected = "no arrangement")]
fn globals_truly_exceeding_total_capacity_still_panic_with_a_clear_message() {
    // 5 x 70-byte globals = 350 bytes > the device's 320-byte total GPR
    // capacity: no arrangement fits, so this must still panic, now with a
    // message naming the real constraint instead of a bare hex address.
    let mut src = String::new();
    for i in 0..5 {
        src.push_str(&format!("global g{i} i8\n"));
    }
    src.push_str("fn main(void) ()\n  block entry:\n    ret void\n");
    let mut m = parse(&src);
    for i in 0..5 {
        m.globals[i].size = 70;
    }
    let _ = allocate(&PIC16F877A, &m, "depth 1\n");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `nix develop --command cargo test -p alloc a_global_layout_sequential_placement_cannot_fit_succeeds_via_bin_packing globals_truly_exceeding_total_capacity`
Expected: FAIL — the first test panics (today's sequential-only globals loop hasn't been given the fallback yet); the second test fails because the panic message doesn't yet contain `"no arrangement"` (today's message is `"alloc: GPR demand exceeds 0x..."`).

- [ ] **Step 3: Write the implementation**

In `allocate()`, change the globals step's final failure path from directly panicking inside `try_place_globals_sequential`'s call site to:

```rust
    let globals: HashMap<String, u16> = try_place_globals_sequential(device, &non_const)
        .or_else(|| place_globals_bin_packed(device, &non_const))
        .unwrap_or_else(|| {
            let demand: u32 = non_const.iter().map(|g| u32::from(g.size)).sum();
            let capacity: u32 =
                device.ram_banks.iter().map(|&(s, e)| u32::from(e) - u32::from(s) + 1).sum();
            let bank_count = device.ram_banks.len();
            panic!(
                "alloc: no arrangement of {} global(s) fits {}'s {bank_count} GPR bank window(s) \
                 (total demand {demand} bytes, total capacity {capacity} bytes — every arrangement, \
                 including largest-first bin-packing, leaves at least one global with no single bank \
                 window big enough for it)",
                non_const.len(),
                device.name,
            );
        });
```

(Exact wording is not load-bearing beyond containing `"no arrangement"`, per Task 3's own test — adjust phrasing for clarity as needed, but keep it naming the real constraint rather than reading like a generic out-of-memory message.)

Update the module-level doc comment at the top of `crates/alloc/src/lib.rs` to mention the two-phase strategy: after the existing "Globals get sequential, even-aligned (i16) addresses..." sentence, add a sentence noting that a bin-packing fallback (largest-first, independent per-bank cursors) activates only when sequential placement would otherwise fail, so successfully-allocated programs keep unchanged addresses.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nix develop --command cargo test -p alloc`
Expected: PASS — every test in the crate, old and new.

- [ ] **Step 5: Run the full workspace test suite**

Run: `nix develop --command cargo test --workspace`
Expected: PASS with zero failures. This is the real proof the "byte-for-byte identical addresses for every currently-succeeding program" global constraint holds — any existing `isel`/`driver`/`banking`/e2e test with hard-coded global addresses would fail here if the sequential path's behavior changed in any way for inputs it already handles.

- [ ] **Step 6: Commit**

```bash
git add crates/alloc/src/lib.rs crates/alloc/tests/alloc.rs
git commit -m "fix(alloc): fall back to bin-packing when sequential global placement fails (#7)"
```

---

## Self-Review

**Spec coverage:** issue #7's ask ("place FSR-accessed globals inside single bank windows... place FSR-accessed globals so they never straddle a window boundary, and only panic when there is genuinely no room") is covered end to end: `try_place_at`/`try_place_globals_sequential` (Task 1) preserve today's straddle-avoiding placement exactly; `place_globals_bin_packed` (Task 2) adds a strictly-better arrangement search; Task 3's wiring means a program only panics when NEITHER a sequential NOR a largest-first-packed arrangement fits — genuinely, not just an artifact of declaration order — with a message that names the real constraint. The user's chosen direction (bin-pack FSR-needing objects across windows, matching the worked example in the approval step) is implemented via First-Fit-Decreasing, the direct algorithmic match for that example.

**Placeholder scan:** every task has complete, concrete code (no `TODO`/`fill in later`); the one explicit hedge ("adjust phrasing as needed" for the final panic message) is scoped to non-load-bearing wording, with the load-bearing substring (`"no arrangement"`) pinned by Task 3's own test.

**Type consistency:** `try_place_at`, `try_place_globals_sequential`, and `place_globals_bin_packed` all take `&Device` first and return `Option<...>` consistently across Tasks 1-3; `BankCursor` (Task 2) is used only within `place_globals_bin_packed`, never exposed; `allocate()`'s call site (Task 3) chains `.or_else(...)` between the two `Option`-returning functions exactly as their signatures allow, with no type mismatch.

**Arithmetic verification:** the reproduction case (three 76-byte globals, one 78-byte global, one 4-byte global — 310 bytes total, under the device's 320-byte capacity) was hand-traced twice during design, in plain decimal, after an earlier hex-based draft of this same trace mixed up "four 76-byte globals + one 78-byte" (382 bytes — that set alone exceeds total device capacity and would fail under *any* arrangement, which would not have demonstrated bin-packing's advantage at all). The corrected, re-verified trace: under a single monotonic cursor placing `g0..g3` in declared order (76, 76, 76, 78), each of the first three 76-byte globals claims a fresh bank leaving a 4-byte leftover behind it (abandoned the moment the cursor advances), and the 78-byte global claims the fourth (last) bank leaving only 2 bytes there; the trailing 4-byte global then fits neither the last bank's 2-byte remainder nor any bank beyond it (there is no fifth bank), so `try_place_globals_sequential` returns `None` / `place_at`'s underlying loop would panic past 0x1EF. Under First-Fit-Decreasing, the 78-byte global (largest) is placed first and claims bank 0 (leaving a 2-byte leftover there instead of 4), each 76-byte global then claims its own bank in turn (banks 1-3), and the trailing 4-byte global fits into bank 1's 4-byte leftover (from the 76-byte global placed there, which itself left exactly 4 bytes) — every global placed, arrangement succeeds. Task 1/2's unit tests and Task 3's integration test verify this via *structural* assertions (no overlaps, every placement within one bank, all 5 succeed) rather than hard-coded hex addresses, so a further small arithmetic slip in this derivation cannot make the plan's own tests wrong the way a hard-coded expected address could — the implementer should still watch the RED failure message in Task 1/3 Step 2 to confirm each test fails for the *expected* reason (sequential returning `None`/panicking, not a compile error or an unrelated failure) before moving to GREEN.
