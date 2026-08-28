//! epic-cc#144 regression: a NULL function argument compiles and runs.
//! clang -O1 prints `ptr noundef null` for a NULL pointer argument;
//! irparse's call-arg whitelist accepted poison but not null, so the
//! build panicked `call arg must carry a value`. parse_val already maps
//! null to Const(0), so the arg parser just had to forward it.

use std::process::Command;

#[test]
fn null_function_arg_compiles_and_runs() {
    let hex_path = std::env::temp_dir().join(format!("null_arg_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/null_arg.c")
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
    sim.run(50_000);
    // g_out: the first global after the (4-byte) g_cb pointer; locate via
    // the allocator the same way the other e2e tests do.
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let tmp = std::env::temp_dir().join(format!("nullarg-{}", std::process::id()));
    let header_dir = tmp.join("include");
    std::fs::create_dir_all(&header_dir).expect("create header dir");
    std::fs::write(header_dir.join("stdint.h"), driver::stdint_h::STDINT_H)
        .expect("write stdint.h");
    let opts = driver::clang::Options {
        includes: Vec::new(),
        defines: Vec::new(),
        header_dir: Some(header_dir),
        fosc_hz: None,
        packed_structs: false,
    };
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new("tests/fixtures/null_arg.c"),
        &opts,
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let out_addr = *layout.globals.get("g_out").expect("g_out") as usize;
    assert_eq!(sim.ram()[out_addr], 7, "g_out == 7 for a NULL callback");
}
