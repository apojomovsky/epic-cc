//! Maybe-flash pointer joins on this core (epic-cc#1347): a pointer
//! joining a flash literal with a RAM buffer reads the literal's RAM
//! copy on the flash arm, so every shape below runs both arms in sim.

use asm::assemble_words;
use banking::assign_banks;
use device::PIC16F877A;
use isel::select;
use peephole::optimize;
use pic14_sim::Pic14;
use schedule::schedule;
use std::collections::HashMap;
use std::process::Command;

fn compile(c_path: &str) -> (Pic14, HashMap<String, u16>) {
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
    let out = Command::new(&clang)
        .args([
            "-target",
            "msp430",
            "-O1",
            "-S",
            "-emit-llvm",
            "-ffreestanding",
            "-nostdinc",
            "-g",
            "-resource-dir",
            &resdir,
            "-o",
            "-",
            c_path,
        ])
        .output()
        .expect("run clang");
    assert!(
        out.status.success(),
        "clang: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ll = String::from_utf8(out.stdout).unwrap();
    let mut m = irparse::parse_ll(&ll);
    m = wholeprog::merge(m);
    m = legalize::legalize(m);
    let cg = callgraph::build(&m);
    callgraph::check_depth(&cg, PIC16F877A.stack_depth as usize);
    let layout = alloc::allocate(&PIC16F877A, &m, &callgraph::edges_text(&cg));
    let mut addrs: HashMap<String, u16> = HashMap::new();
    addrs.extend(layout.globals.clone());
    addrs.extend(layout.locals.clone());
    let asm = select(&PIC16F877A, &m, &addrs);
    let asm = schedule(&PIC16F877A, &asm);
    let asm = assign_banks(&PIC16F877A, &asm);
    let asm = optimize(&asm);
    isel::verify_page_fit(&m, &asm);
    let words = assemble_words(&PIC16F877A, &asm);
    (Pic14::with_device(&PIC16F877A, words), layout.globals)
}

macro_rules! shape {
    ($name:ident, $file:expr) => {
        shape!($name, $file, b'Q');
    };
    ($name:ident, $file:expr, $flash:expr) => {
        #[test]
        fn $name() {
            let (mut p, globals) = compile(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/",
                $file
            ));
            p.ram_mut()[globals["flag"] as usize] = 0;
            p.run(2_000_000);
            assert!(p.halted());
            assert_eq!(p.ram()[globals["out"] as usize], $flash, "flash arm");
            let (mut p, globals) = compile(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/",
                $file
            ));
            p.ram_mut()[globals["flag"] as usize] = 1;
            p.run(2_000_000);
            assert!(p.halted());
            assert_eq!(p.ram()[globals["out"] as usize], b'b', "ram arm");
        }
    };
}

shape!(mf_select, "mf_select.c");
shape!(mf_phi, "mf_phi.c");
shape!(mf_global, "mf_global.c");
shape!(mf_global_init, "mf_global_init.c");
shape!(mf_select_param, "mf_select_param.c");
shape!(mf_return, "mf_return.c");

shape!(mf_select_big, "mf_select_big.c", b'A');

#[test]
fn mf_store() {
    for flag in [0u8, 1] {
        let (mut p, globals) = compile(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mf_store.c"
        ));
        p.ram_mut()[globals["flag"] as usize] = flag;
        p.run(2_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[globals["out"] as usize], b'Z', "flag={flag}");
    }
}

#[test]
fn mf_index() {
    for (flag, want) in [(0u8, b'R'), (1u8, b'c')] {
        let (mut p, globals) = compile(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mf_index.c"
        ));
        p.ram_mut()[globals["flag"] as usize] = flag;
        p.ram_mut()[globals["idx"] as usize] = 2;
        p.run(2_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[globals["out"] as usize], want, "flag={flag}");
    }
}

#[test]
fn mf_memcpy() {
    for (flag, want) in [(0u8, b'Q'), (1u8, b'b')] {
        let (mut p, globals) = compile(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mf_memcpy.c"
        ));
        p.ram_mut()[globals["flag"] as usize] = flag;
        p.run(2_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[globals["out"] as usize], want, "flag={flag}");
    }
}

#[test]
fn mf_wide() {
    for (flag, want) in [(0u8, 0x52u8), (1u8, 0x43u8)] {
        let (mut p, globals) = compile(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mf_wide.c"
        ));
        p.ram_mut()[globals["flag"] as usize] = flag;
        p.run(2_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[globals["out"] as usize], want, "flag={flag}");
    }
}

#[test]
fn mf_pure_global_init() {
    let (mut p, globals) = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mf_pure_global_init.c"
    ));
    p.run(2_000_000);
    assert!(p.halted());
    assert_eq!(p.ram()[globals["out"] as usize], b'Q');
}

#[test]
#[should_panic(expected = "no RAM copy on this core")]
fn mf_global_big_init() {
    let _ = compile(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mf_global_big_init.c"
    ));
}

#[test]
fn mf_intjoin() {
    for (flag, want) in [(0u8, b'Q'), (1u8, b'b')] {
        let (mut p, globals) = compile(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mf_intjoin.c"
        ));
        p.ram_mut()[globals["flag"] as usize] = flag;
        p.run(2_000_000);
        assert!(p.halted());
        assert_eq!(p.ram()[globals["out"] as usize], want, "flag={flag}");
    }
}
