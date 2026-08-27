//! epic-cc#160 regression: llvm.umin and llvm.usub.sat lower correctly.
//! clang -O1 emits llvm.umin for the priority min-finding loop and
//! llvm.usub.sat for the guarded decrement of the memory value; legalize
//! now lowers both (icmp ult + select, icmp uge + sub + select).

use std::process::Command;

#[test]
fn umin_and_usub_sat_run_correctly() {
    let hex_path = std::env::temp_dir().join(format!("umin_usub_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/umin_usub.c")
        .output()
        .expect("run epic-cc");
    assert!(
        out.status.success(),
        "epic-cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let produced = std::fs::read_to_string(&hex_path).expect("read hex");
    let _ = std::fs::remove_file(&hex_path);
    let prog = pic14_sim::parse_hex(&produced);
    let mut sim = pic14_sim::Pic14::new(prog);
    sim.run(100_000);
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let tmp = std::env::temp_dir().join(format!("uminusub-{}", std::process::id()));
    let header_dir = tmp.join("include");
    std::fs::create_dir_all(&header_dir).expect("create header dir");
    std::fs::write(header_dir.join("stdint.h"), driver::stdint_h::STDINT_H)
        .expect("write stdint.h");
    let opts = driver::clang::Options {
        includes: Vec::new(),
        defines: Vec::new(),
        header_dir: Some(header_dir),
        fosc_hz: None,
    };
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/umin_usub.c"),
        &opts,
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let min_addr = *layout.globals.get("g_min").expect("g_min") as usize;
    let count_addr = *layout.globals.get("g_count").expect("g_count") as usize;
    assert_eq!(sim.ram()[min_addr], 0, "min of 3,1,2,0 == 0");
    assert_eq!(sim.ram()[count_addr], 1, "guarded decrement of 2 == 1");
}
