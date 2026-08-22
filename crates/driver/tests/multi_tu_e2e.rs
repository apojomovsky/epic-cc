//! CC-1 acceptance: three translation units, colliding statics (`bump`,
//! `scratch` defined identically in both `multi_tu_a.c` and `multi_tu_b.c`),
//! a global defined in one unit and written from two others (`total`), and
//! cross-unit calls. Proves the real `epic-cc` binary's `llvm-link` merge and
//! `irparse::sanitize_symbols` end to end, not just the units in isolation.

use std::process::Command;

const INPUTS: [&str; 3] = [
    "tests/fixtures/multi_tu_main.c",
    "tests/fixtures/multi_tu_a.c",
    "tests/fixtures/multi_tu_b.c",
];

/// Reproduce the driver's own front half (clang per unit, llvm-link merge,
/// sanitize) plus `alloc`, so the test can find `total`'s RAM address the
/// same way `array_e2e.rs` and `banked_e2e.rs` do for a single unit.
fn multi_tu_layout() -> alloc::AllocLayout {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let llvm_link = driver::clang_discovery::resolve_llvm_link(&clang).expect("resolve_llvm_link");

    let tmp = std::env::temp_dir().join(format!("epiccc-multi-tu-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let mut units = Vec::new();
    for (n, input) in INPUTS.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        driver::clang::compile_to_file(
            &clang,
            &resdir,
            std::path::Path::new(input),
            &ll_path,
            &driver::clang::Options::default(),
        );
        units.push(ll_path);
    }

    let merged_path = tmp.join("merged.ll");
    let mut cmd = Command::new(&llvm_link);
    cmd.arg("-S");
    for u in &units {
        cmd.arg(u);
    }
    cmd.args(["-o", merged_path.to_str().unwrap()]);
    let out = cmd.output().expect("run llvm-link");
    assert!(
        out.status.success(),
        "llvm-link: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ll_text =
        irparse::sanitize_symbols(&std::fs::read_to_string(&merged_path).expect("read merged .ll"));
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg))
}

#[test]
fn compiles_three_translation_units_end_to_end() {
    let layout = multi_tu_layout();
    let total_addr = *layout.globals.get("total").expect("total global") as usize;

    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/multi_tu_main.c",
            "tests/fixtures/multi_tu_a.c",
            "tests/fixtures/multi_tu_b.c",
            "-o",
            "tests/fixtures/multi_tu.hex",
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

    let hex = std::fs::read_to_string("tests/fixtures/multi_tu.hex").unwrap();
    let prog = pic14_sim::parse_hex(&hex);
    let mut p = pic14_sim::Pic14::new(prog);
    p.run(2000);
    assert!(p.halted());
    // from_a(3): scratch = 3, return scratch + 1 = 4.
    // from_b(4): scratch = 4, return scratch + 2 = 6.
    // total = 4 + 6 = 10 (0x0A).
    assert_eq!(p.ram()[total_addr], 0x0A);
}

/// This test exists to catch the acceptance test above becoming vacuous: if
/// clang ever manages to inline `bump`/`scratch` away (see the `noinline`
/// and `volatile` note on the fixtures), llvm-link never renames anything
/// and the merge path this whole task exists for goes untested.
#[test]
fn the_merge_actually_renamed_colliding_symbols() {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let llvm_link = driver::clang_discovery::resolve_llvm_link(&clang).expect("resolve_llvm_link");

    let tmp =
        std::env::temp_dir().join(format!("epiccc-multi-tu-collision-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let mut units = Vec::new();
    for (n, input) in INPUTS.iter().enumerate() {
        let ll_path = tmp.join(format!("{n:03}.ll"));
        driver::clang::compile_to_file(
            &clang,
            &resdir,
            std::path::Path::new(input),
            &ll_path,
            &driver::clang::Options::default(),
        );
        units.push(ll_path);
    }

    let merged_path = tmp.join("merged.ll");
    let mut cmd = Command::new(&llvm_link);
    cmd.arg("-S");
    for u in &units {
        cmd.arg(u);
    }
    cmd.args(["-o", merged_path.to_str().unwrap()]);
    let out = cmd.output().expect("run llvm-link");
    assert!(out.status.success());

    let merged = std::fs::read_to_string(&merged_path).unwrap();
    let bump_count = merged.lines().filter(|l| l.contains("@bump")).count();
    let scratch_count = merged.lines().filter(|l| l.contains("@scratch")).count();
    assert!(
        bump_count >= 2 && scratch_count >= 2,
        "expected llvm-link to keep two distinct @bump/@scratch symbols (one renamed), \
         found {bump_count} bump line(s) and {scratch_count} scratch line(s) in:\n{merged}"
    );
    let dotted_renames = merged
        .lines()
        .filter(|l| l.contains('@') && l.contains('.'))
        .count();
    assert!(
        dotted_renames > 0,
        "expected at least one dotted llvm-link rename (e.g. @bump.3) before sanitizing"
    );
}
