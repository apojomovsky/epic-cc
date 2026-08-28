//! Cross-context dispatch (the epic-taskmgr shape under epic-cc,
//! epic-hal#86): a task table whose fn field is stored by main while
//! the ISR reads only the flags/countdown fields. Before the
//! field-sensitive ISR-read fix, the whole-table read pulled the stored
//! task into the ISR context, the main dispatch lost its candidate and
//! the fn call trapped; before the width filter, the 1-arg i8 RB site
//! collected the 1-arg ptr-param task and isel panicked copying a
//! narrow arg into a 2-byte slot.

use std::process::Command;

#[test]
fn cross_context_dispatch_runs_the_task() {
    let hex_path = std::env::temp_dir().join(format!("cb_dispatch_{}.hex", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--target", "16F877A", "-o"])
        .arg(&hex_path)
        .arg("tests/fixtures/cb_dispatch.c")
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
    let tmp = std::env::temp_dir().join(format!("cbdisp-{}", std::process::id()));
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
        std::path::Path::new("tests/fixtures/cb_dispatch.c"),
        &opts,
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let mut m = irparse::parse_ll(&ll_text);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    let layout = alloc::allocate(&device::PIC16F877A, &m, &callgraph::edges_text(&cg));
    let seen_addr = *layout.globals.get("g_seen").expect("g_seen") as usize;
    let rb_addr = *layout.globals.get("g_rb").expect("g_rb") as usize;
    assert_eq!(
        sim.ram()[seen_addr],
        42,
        "task ran with its arg (main dispatch)"
    );
    assert_eq!(
        sim.ram()[rb_addr],
        0xAB,
        "RB callback site dispatched the i8 callback"
    );
}
