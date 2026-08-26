# Size and Map Reporting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The driver prints a size report to stderr after every hex build and writes a symbol-to-address map file on `--map <file>`, closing CC-6's reporting half (`epic-cc#74`).

**Architecture:** `alloc` already holds every RAM fact; it gains per-bank high-water marks and the ISR region span. `asm` gains a program-words helper. The driver captures the program words before config insertion, writes the map from `alloc::map_text`, and prints a size report whose "RAM used" definition is stated on the line.

**Tech Stack:** Rust workspace (no external crates), docker dev image (`make exec`).

**Spec:** `docs/superpowers/specs/2026-08-26-size-map-reporting-design.md`

## Global Constraints

- No external crates; std only.
- Conventional Commits, single line, no trailers, no em-dashes.
- Comments: why not what, 1-8 line blocks, no decoration, no narrative.
- `make check-warnings` must stay clean (rustc lints are a hard gate).
- The map file format is `alloc::map_text`'s existing contract: `global <name> 0xNN`, `local <func> <name> 0xNN`, `const <name>`.
- Size report goes to stderr, unconditional on hex builds (D-4 config report precedent).
- Tests run in the docker image: `make test CRATE=driver` / `make exec CMD='cargo test ...'`.

---

### Task 1: AllocLayout gains bank_used and isr_bytes

**Files:**
- Modify: `crates/alloc/src/lib.rs` (struct at :28-34, `allocate` tail at :639-661)
- Test: `crates/alloc/tests/alloc.rs`

**Interfaces:**
- Consumes: `device.ram_banks: &'static [(u16, u16)]`, `device.gpr_start()`, the existing `globals`/`locals` maps and `base`/`locals_widths`/`isr_ctx` locals inside `allocate`.
- Produces: `AllocLayout { ..., pub bank_used: Vec<u16>, pub isr_bytes: u16 }` — `bank_used[i]` = high-water bytes in `ram_banks[i]` (both contexts), `isr_bytes` = the disjoint ISR region's span (0 without an ISR).

- [ ] **Step 1: Write the failing tests**

In `crates/alloc/tests/alloc.rs`, add:

```rust
#[test]
fn bank_used_tracks_high_water_per_bank() {
    // 79 i8 globals fill bank 0 GPR (0x20..0x6E); main's i16 local moves
    // wholesale to 0xA0 (bank 1) leaving a 1-byte hole at 0x6F, and its
    // i8 local lands at 0xA2. bank_used[0] = 0x6F - 0x20 + 1 = 80,
    // bank_used[1] = 0xA3 - 0xA0 = 3, banks 2-3 = 0.
    let mut gsrc = String::new();
    for i in 0..79 {
        gsrc.push_str(&format!("global g{i} i8\n"));
    }
    let m = parse(
        &format!(
            "{gsrc}fn main(void) ()\n\
               block entry:\n\
                 %v0 = add i16 1, 2\n\
                 %v1 = add i8 3, 4\n\
                 ret void\n"
        ),
    );
    let out = allocate(&PIC16F877A, &m, "depth 1\n");
    assert_eq!(out.bank_used, vec![80, 3, 0, 0]);
    assert_eq!(out.isr_bytes, 0);
}

#[test]
fn isr_bytes_reports_the_disjoint_region_span() {
    // main's context occupies 0x20..0x23 (depth_end 3); the ISR root's
    // base is 0x23 and its chain (isr -> m1_isr -> m2_isr, one i8 local
    // each) ends at 0x26. isr_bytes = 0x26 - 0x23 = 3.
    let m = parse(
        "fn main(void) ()\n\
           block entry:\n\
             %v0 = add i8 1, 2\n\
             call void @m1()\n\
             ret void\n\
         fn m1(void) ()\n\
           block entry:\n\
             %v1 = add i8 1, 2\n\
             call void @m2()\n\
             ret void\n\
         fn m2(void) ()\n\
           block entry:\n\
             %v2 = add i8 1, 2\n\
             ret void\n\
         fn isr(void) [isr] ()\n\
           block entry:\n\
             %i0 = add i8 1, 2\n\
             call void @m1_isr()\n\
             ret void\n\
         fn m1_isr(void) ()\n\
           block entry:\n\
             %i1 = add i8 1, 2\n\
             call void @m2_isr()\n\
             ret void\n\
         fn m2_isr(void) ()\n\
           block entry:\n\
             %i2 = add i8 1, 2\n\
             ret void\n",
    );
    let out = allocate(
        &PIC16F877A,
        &m,
        "edge main m1\nedge m1 m2\nedge isr m1_isr\nedge m1_isr m2_isr\n",
    );
    assert_eq!(out.isr_bytes, 3);
    // The ISR region is included in the bank totals: the highest ISR
    // address 0x25 is in bank 0, so bank_used[0] = 0x26 - 0x20 = 6.
    assert_eq!(out.bank_used[0], 6);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `make exec CMD='cargo test -p alloc --test alloc bank_used_tracks_high_water_per_bank isr_bytes_reports_the_disjoint_region_span'`
Expected: FAIL, `no field bank_used` / `no field isr_bytes`.

- [ ] **Step 3: Implement**

In `crates/alloc/src/lib.rs`:

```rust
pub struct AllocLayout {
    pub globals: HashMap<String, u16>,
    pub locals: HashMap<String, u16>,
    pub total_bank0: u16,
    pub const_globals: HashSet<String>,
    /// Per-bank high-water bytes (both main and ISR contexts): the highest
    /// allocated address in each GPR bank minus the bank start, floored at
    /// 0. The allocator places sequentially from each bank start, so this
    /// is the occupied bytes; the only holes are the 1-byte region-tail
    /// gaps an i16 leaves when it moves wholesale to the next bank, which
    /// the high-water mark conservatively includes.
    pub bank_used: Vec<u16>,
    /// The disjoint ISR region's span in bytes (0 without an ISR): the
    /// distance from the ISR root's base to the highest ISR-context frame
    /// end. Reported separately and included in `bank_used`.
    pub isr_bytes: u16,
}
```

In `allocate`, after the locals loop (step 7) and before `total_bank0` (step 8), add:

```rust
    // 7b. Per-bank high-water marks and the ISR region span. Every placed
    // address (globals + locals, both contexts) contributes its end; the
    // ISR region is the distance from the ISR root's base to the highest
    // ISR-context frame end, 0 without an ISR.
    let mut bank_used: Vec<u16> = device.ram_banks.iter().map(|_| 0u16).collect();
    let mut isr_bytes: u16 = 0;
    let mut isr_ctx: HashSet<String> = HashSet::new();
    if !isr_names.is_empty() {
        let isr_roots: Vec<&String> = topo
            .iter()
            .filter(|f| !callers.contains_key(*f) && isr_names.contains(f.as_str()))
            .collect();
        isr_ctx = isr_roots
            .iter()
            .flat_map(|r| reachable(&[r.as_str()], &edges))
            .collect();
        let isr_lo = isr_roots
            .iter()
            .map(|r| base[r])
            .min()
            .unwrap_or(bank0_start);
        let isr_hi = isr_ctx
            .iter()
            .map(|f| frame_end(device, base[f], &locals_widths[f]))
            .max()
            .unwrap_or(isr_lo);
        isr_bytes = isr_hi - isr_lo;
    }
    for (i, &(start, _)) in device.ram_banks.iter().enumerate() {
        let mut hi = start;
        for (_, &a) in globals.iter().chain(locals.iter()) {
            if a >= start {
                hi = hi.max(a);
            }
        }
        bank_used[i] = hi - start + 1;
    }
```

Then extend the struct literal:

```rust
    AllocLayout {
        globals,
        locals,
        total_bank0,
        const_globals,
        bank_used,
        isr_bytes,
    }
```

Note: `isr_ctx` is already computed inside the `if !isr_names.is_empty()` block at step 6b; the new code recomputes it (the existing one is scoped inside that block). Keep the new computation self-contained.

- [ ] **Step 4: Run to verify they pass**

Run: `make exec CMD='cargo test -p alloc'`
Expected: PASS (new tests + all existing alloc tests).

- [ ] **Step 5: Commit**

```bash
git add crates/alloc/src/lib.rs crates/alloc/tests/alloc.rs
git commit -m "feat(alloc): report per-bank usage and the ISR region span"
```

---

### Task 2: asm gains assemble_words

**Files:**
- Modify: `crates/asm/src/lib.rs` (`assemble_file_to_hex` at :462-477)
- Test: `crates/asm/tests/assemble.rs`

**Interfaces:**
- Consumes: `device.core`, `device.flash_words`, `assemble`, `assemble_pic18`.
- Produces: `pub fn assemble_words(device: &Device, src: &str) -> Vec<u16>` — the program words with the flash-size assert; `assemble_file_to_hex` delegates to it.

- [ ] **Step 1: Write the failing test**

In `crates/asm/tests/assemble.rs`:

```rust
#[test]
fn assemble_words_returns_program_words_without_hex() {
    let src = "    org 0x0000\n    movlw 0x2A\n    nop\n    end\n";
    let words = assemble_words(&device::PIC16F877A, src);
    assert_eq!(words.len(), 2);
    assert_eq!(words[0], 0x302A); // movlw 0x2A
    assert_eq!(words[1], 0x0000); // nop
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `make exec CMD='cargo test -p asm --test assemble assemble_words_returns_program_words_without_hex'`
Expected: FAIL, `cannot find function assemble_words`.

- [ ] **Step 3: Implement**

In `crates/asm/src/lib.rs`, replace `assemble_file_to_hex` with:

```rust
/// Assemble source into program words, asserting the program fits the
/// device's flash. The driver uses this for both the size report and the
/// hex emission, so the reported flash count is exactly the program's.
pub fn assemble_words(device: &Device, src: &str) -> Vec<u16> {
    let words = match device.core {
        device::Core::Pic14 => assemble(src),
        device::Core::Pic18 => assemble_pic18(src),
        device::Core::Pic14e => panic!("asm: pic14e core not yet implemented for {}", device.name),
    };
    assert!(
        words.len() as u32 <= device.flash_words,
        "asm: program of {} words exceeds device flash (highest address 0x{:04X} >= {:#06x}; {}-word flash)",
        words.len(),
        words.len().saturating_sub(1),
        device.flash_words,
        device.flash_words,
    );
    words
}

/// Assemble source and render the result as Intel HEX.
///
/// The whole program (code + tables) must fit the device's flash: a program
/// whose highest word address is beyond `device.flash_words` panics loudly.
/// `assemble`/`assemble_pic18` are layout-only and stay unasserted so
/// isel's unit tests can inspect words of any size.
pub fn assemble_file_to_hex(device: &Device, src: &str) -> String {
    to_hex(&assemble_words(device, src))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `make exec CMD='cargo test -p asm'`
Expected: PASS (new test + all existing asm tests, including the gpasm cross-checks).

- [ ] **Step 5: Commit**

```bash
git add crates/asm/src/lib.rs crates/asm/tests/assemble.rs
git commit -m "feat(asm): expose assemble_words for the size report"
```

---

### Task 3: CLI gains --map

**Files:**
- Modify: `crates/driver/src/cli.rs`
- Test: `crates/driver/tests/cli.rs`

**Interfaces:**
- Consumes: the existing `parse_args` loop.
- Produces: `Cli { ..., pub map: Option<String> }`; `--map <file>` parses like `--save-temps`.

- [ ] **Step 1: Write the failing test**

In `crates/driver/tests/cli.rs`:

```rust
#[test]
fn parses_a_map_file() {
    let c = parse_args(&args(&[
        "a.c",
        "--device",
        "p16f877a",
        "--map",
        "out.map",
    ]))
    .unwrap();
    assert_eq!(c.map.as_deref(), Some("out.map"));
}

#[test]
fn map_defaults_to_none() {
    let c = parse_args(&args(&["a.c", "--device", "p16f877a"])).unwrap();
    assert_eq!(c.map, None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `make exec CMD='cargo test -p driver --test cli parses_a_map_file map_defaults_to_none'`
Expected: FAIL, `no field map`.

- [ ] **Step 3: Implement**

In `crates/driver/src/cli.rs`:

- Add `pub map: Option<String>,` to `Cli`.
- Add to USAGE, after the `--save-temps` line:

```
  --map <file>         write the symbol-to-address map (globals and
                       {func}::{name} locals) into <file>
```

- In `parse_args`: add `let mut map = None;`, and in the loop:

```rust
        } else if a == "--map" {
            i += 1;
            map = Some(
                argv.get(i)
                    .cloned()
                    .ok_or("epic-cc: --map needs a value")?,
            );
        }
```

- In the `Ok(Cli { ... })` literal: add `map,`.

- [ ] **Step 4: Run to verify they pass**

Run: `make exec CMD='cargo test -p driver --test cli'`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/driver/src/cli.rs crates/driver/tests/cli.rs
git commit -m "feat(driver): parse --map for the address map file"
```

---

### Task 4: report module and driver wiring

**Files:**
- Create: `crates/driver/src/report.rs`
- Modify: `crates/driver/src/lib.rs` (add `pub mod report;`), `crates/driver/src/main.rs`
- Test: `crates/driver/tests/size_map_e2e.rs` (new)

**Interfaces:**
- Consumes: `device::Device`, `alloc::AllocLayout`, `asm::assemble_words`, `alloc::map_text`, `cli::Cli`.
- Produces: `report::render_size(device, layout, flash_used) -> String`; the driver writes the map and prints the report.

- [ ] **Step 1: Write the failing e2e test**

Create `crates/driver/tests/size_map_e2e.rs`:

```rust
//! CC-6 reporting acceptance: the size report on stderr matches the HEX and
//! the allocator's layout, and --map writes the allocator's map text.

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("epic-cc-size-map-{}-{name}", std::process::id()));
    p
}

fn fixture_add() -> String {
    format!("{}/tests/fixtures/add.c", env!("CARGO_MANIFEST_DIR"))
}

/// Run the full pipeline on add.c exactly as the driver does and return the
/// alloc layout, so the report can be checked against the allocator's own
/// facts.
fn add_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/add.c"),
        &driver::clang::Options::default(),
    );
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg))
}

#[test]
fn size_report_matches_hex_and_layout() {
    let hex_path = tmp("add.hex");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Flash: count the program words in the HEX (the highest nonzero word
    // address + 1, the same trim to_hex applies).
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let flash_used = prog
        .iter()
        .rposition(|&w| w != 0)
        .map(|i| i + 1)
        .unwrap_or(0);

    let layout = add_layout();
    let report = String::from_utf8_lossy(&out.stderr);
    assert!(
        report.contains(&format!("flash: {flash_used}/8192 words")),
        "flash line missing or wrong: {report}"
    );
    // RAM: the report's bank lines must match the layout's bank_used and
    // the device's bank sizes.
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device::PIC16F877A.ram_banks[i];
        let total = end - start + 1;
        assert!(
            report.contains(&format!("bank {i}: {used}/{total} bytes")),
            "bank {i} line missing or wrong: {report}"
        );
    }
    // The report states what it means by used.
    assert!(
        report.contains("overlay"),
        "RAM line must state the overlay definition: {report}"
    );
    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn map_file_matches_the_allocator_map() {
    let hex_path = tmp("map.hex");
    let map_path = tmp("add.map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            &fixture_add(),
            "-o",
            hex_path.to_str().unwrap(),
            "--device",
            "p16f877a",
            "--map",
            map_path.to_str().unwrap(),
        ])
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let layout = add_layout();
    let written = std::fs::read_to_string(&map_path).unwrap();
    assert_eq!(written, alloc::map_text(&layout));
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `make exec CMD='cargo test -p driver --test size_map_e2e'`
Expected: FAIL (report not printed, `--map` unknown option).

- [ ] **Step 3: Implement report.rs**

Create `crates/driver/src/report.rs`:

```rust
//! The size report the driver prints to stderr after every hex build.
//!
//! "RAM used" is the bytes of RAM the program's allocation occupies: the
//! per-bank high-water marks from the overlay layout plus the fixed
//! scratch/retval/ISR-save region isel reserves. Overlay allocation makes
//! this less obvious than on a stack machine, since a byte can be live in
//! several frames, so the report states the definition on the line.

use alloc::AllocLayout;
use device::Device;

/// The fixed bytes isel reserves outside the overlay: PIC14's common-RAM
/// scratch (1) + retval (4), plus the ISR save area (9) when the program
/// has an ISR. PIC18's access-bank retval/flag region (4), plus the ISR
/// save area (12) when the program has an ISR. These are isel's layout
/// constants (crates/isel/src/lib.rs, crates/isel-pic18/src/lib.rs).
pub fn fixed_bytes(device: &Device, has_isr: bool) -> u16 {
    match device.core {
        device::Core::Pic14 => {
            let base = 1 + 4; // scratch + retval
            if has_isr {
                base + 9
            } else {
                base
            }
        }
        device::Core::Pic18 => {
            let base = 4; // retval + flag bit
            if has_isr {
                base + 12
            } else {
                base
            }
        }
        device::Core::Pic14e => 0,
    }
}

/// The fixed region's total capacity: PIC14 common RAM, PIC18's access
/// bank (the fixed_retval reservation is a policy slice of it).
pub fn fixed_total(device: &Device) -> u16 {
    match device.core {
        device::Core::Pic14 => {
            let (lo, hi) = device
                .common_ram
                .expect("PIC14 devices have a common-RAM region");
            hi - lo + 1
        }
        device::Core::Pic18 => {
            let (lo, hi) = device
                .access_bank
                .expect("PIC18 devices have an access bank");
            hi - lo + 1
        }
        device::Core::Pic14e => 0,
    }
}

/// Render the size report. `flash_used` is the program's assembled word
/// count (before config-word insertion); `layout` carries the RAM facts.
pub fn render_size(device: &Device, layout: &AllocLayout, flash_used: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "epic-cc: program size for {}:\n",
        device.name
    ));
    out.push_str(&format!(
        "  flash: {flash_used}/{} words ({:.1}%)\n",
        device.flash_words,
        flash_used as f64 * 100.0 / device.flash_words as f64
    ));
    let ram_total: u16 = device
        .ram_banks
        .iter()
        .map(|&(s, e)| e - s + 1)
        .sum::<u16>()
        + fixed_total(device);
    let ram_used: u16 = layout.bank_used.iter().sum::<u16>() + fixed_bytes(device, layout.isr_bytes > 0);
    out.push_str(&format!(
        "  RAM: {ram_used}/{ram_total} bytes ({:.1}%) (overlay: a byte can be live in several frames; used = the bytes of RAM the program's allocation occupies)\n",
        ram_used as f64 * 100.0 / ram_total as f64
    ));
    for (i, &used) in layout.bank_used.iter().enumerate() {
        let (start, end) = device.ram_banks[i];
        let total = end - start + 1;
        out.push_str(&format!("    bank {i}: {used}/{total} bytes\n"));
    }
    let fixed = fixed_bytes(device, layout.isr_bytes > 0);
    let fixed_total = fixed_total(device);
    let fixed_name = match device.core {
        device::Core::Pic14 => "common",
        device::Core::Pic18 => "fixed",
        device::Core::Pic14e => "fixed",
    };
    out.push_str(&format!(
        "    {fixed_name}: {fixed}/{fixed_total} bytes (fixed scratch/retval/ISR save)\n"
    ));
    if layout.isr_bytes > 0 {
        out.push_str(&format!(
            "    ISR region: {} bytes (disjoint, after the main context, included in the bank totals)\n",
            layout.isr_bytes
        ));
    }
    out
}
```

- [ ] **Step 4: Wire the driver**

In `crates/driver/src/lib.rs`, add `pub mod report;`.

In `crates/driver/src/main.rs`:

1. After the `--emit asm` early return, replace the hex block's word assembly with `assemble_words`:

```rust
    // 10. asm: assembly -> Intel HEX (with config words when present). The
    // program words are captured before config insertion: the PIC14 config
    // word lives past the flash ceiling (0x2007 on the 877A), so the hex
    // vec is resized to include it and its length would overcount flash.
    let fuse_spec = canonical_spec
        .as_deref()
        .map(|s| driver::fosc::fuse_spec(s))
        .unwrap_or_default();
    let config_bytes: Option<Vec<u8>> = if canonical_spec.is_some() {
        Some(device::resolve_config(&device.config, &fuse_spec))
    } else {
        None
    };
    let program_words = asm::assemble_words(device, &asm);
    let hex = match (device.core, &config_bytes) {
        (device::Core::Pic14, Some(cb)) => {
            let mut words = program_words.clone();
            let idx = (device.config.base_byte_addr / 2) as usize;
            if words.len() <= idx {
                words.resize(idx + 1, 0);
            }
            let w = u16::from(cb[0]) | (u16::from(cb[1]) << 8);
            words[idx] = w;
            asm::to_hex(&words)
        }
        (device::Core::Pic18, Some(cb)) => {
            let mut config_words = Vec::new();
            for chunk in cb.chunks(2) {
                let lo = chunk[0] as u16;
                let hi = if chunk.len() > 1 { chunk[1] as u16 } else { 0 };
                config_words.push(lo | (hi << 8));
            }
            asm::to_hex_regions(&[
                (0, &program_words),
                (device.config.base_byte_addr, &config_words),
            ])
        }
        _ => asm::to_hex(&program_words),
    };
```

2. After the config report, add the map write and the size report:

```rust
    if let Some(map_path) = &cli.map {
        std::fs::write(map_path, alloc::map_text(&layout)).expect("write map");
    }
    eprint!("{}", driver::report::render_size(&device, &layout, program_words.len()));
    std::fs::write(&cli.output, hex).expect("write hex");
```

Note: `program_words.len()` is the flash count; `to_hex` trims trailing zero words, so a program whose last word is zero reports the trimmed count, matching the HEX.

- [ ] **Step 5: Run to verify they pass**

Run: `make exec CMD='cargo test -p driver --test size_map_e2e'`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/driver/src/report.rs crates/driver/src/lib.rs crates/driver/src/main.rs crates/driver/tests/size_map_e2e.rs
git commit -m "feat(driver): print the size report and write the map file"
```

---

### Task 5: Full verification

- [ ] **Step 1: Run the driver crate suite**

Run: `make test CRATE=driver`
Expected: PASS, including the new cli/size_map tests and every existing e2e test (the report is additive on stderr; no test asserts empty stderr except `version_flag`, which exits before the pipeline).

- [ ] **Step 2: Run the full workspace suite**

Run: `make test`
Expected: PASS (ci-test.sh per-crate table).

- [ ] **Step 3: Check warnings**

Run: `make check-warnings`
Expected: clean.

- [ ] **Step 4: Smoke the CLI by hand**

Run: `make exec CMD='epic-cc tests/fixtures/add.c -o /tmp/add.hex --device p16f877a --map /tmp/add.map'`
Expected: the size report on stderr, `add.hex` written, `add.map` containing `global in 0x20` / `global out 0x21` lines.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix(driver): ..."  # only if a fix was needed
```

---

### Task 6: Docs, ADR, and plan removal

**Files:**
- Modify: `README.md` (CLI/known-gaps), `docs/31-ecosystem-integration-design.md` (§6 landed table)
- Create: `docs/adr/ADR-025-size-map-reporting.md`
- Modify: `docs/03-decisions.md` (index line)
- Delete: `docs/superpowers/plans/2026-08-26-size-map-reporting.md` (this plan)

- [ ] **Step 1: Update README**

In the "Known gaps" section, remove the `.asm` / `.lst` / `.map` bullet's map claim:

```
- **`.asm` / `.lst` / `.map` output** is not yet exposed by the driver, which emits HEX only.
```

becomes

```
- **`.asm` / `.lst` output** is not yet exposed by the driver, which emits HEX only (the map is: `--map <file>`).
```

- [ ] **Step 2: Update docs/31 §6**

In the landed table, change the CC-6 row:

```
| **CC-6** distribution, size and map | Open | #74, plus #118 for the CI consumable artifact |
```

to

```
| **CC-6** distribution, size and map | Done | #74, ADR-025; #118 for the CI consumable artifact |
```

- [ ] **Step 3: Write ADR-025**

Create `docs/adr/ADR-025-size-map-reporting.md` following the ADR-023 template (Status, Decides, Parent, Decision, Rationale, Alternatives rejected, Known trade-offs, Revisit if). Content: the four decisions from the spec (stderr default + `--map`; RAM-used definition; flash = program words before config insertion; `bank_used`/`isr_bytes` in AllocLayout).

- [ ] **Step 4: Add the index line**

In `docs/03-decisions.md`, after the ADR-024 line:

```
- ADR-025: Size and map reporting (stderr size report, --map file, overlay RAM definition), 2026-08-26
```

- [ ] **Step 5: Delete the plan and commit**

```bash
git rm docs/superpowers/plans/2026-08-26-size-map-reporting.md
git add README.md docs/31-ecosystem-integration-design.md docs/adr/ADR-025-size-map-reporting.md docs/03-decisions.md
git commit -m "docs: record the size and map reporting decisions in ADR-025"
```

---

### Task 7: Takeoff ritual and PR

- [ ] **Step 1: Run the takeoff ritual**

Run: `make pre-pr-check TEST=1`
Expected: exit 0 with the full suite green.

- [ ] **Step 2: Open the PR**

```bash
cat <<'EOF' > /tmp/pr_body.md
Closes #74

CC-6 reporting half: the driver prints a size report to stderr after every
hex build and writes the symbol-to-address map on --map <file>.

- Size report: flash words used out of the device total, RAM used out of
  total with the overlay definition stated, per-bank GPR usage, the fixed
  common/access-bank region, and the disjoint ISR region.
- Map file: alloc's existing map_text contract (global/local/const lines),
  addresses exactly the allocator's.
- AllocLayout gains bank_used and isr_bytes; asm gains assemble_words so
  the flash count is the program's own words before config insertion.
EOF
gh pr create --title "feat(driver): size and map reporting" --body-file /tmp/pr_body.md
```

- [ ] **Step 3: Register the review**

Run: `epic-tasks review epic-cc#74 --pr <url>`
