//! #72 acceptance: const array and single globals of a named struct type
//! decode to their initializer bytes and read correctly through the flash
//! readers on both PIC14 (p16f877a) and PIC18 (p18f4550).

use std::process::Command;

/// `in` and `out`'s RAM addresses, read off the compiler's own `--map`
/// output. Rebuilding the pipeline here instead would be a second copy of
/// `main.rs` that silently drifts: the PIC18 path alone parses with
/// switches preserved, and once the frames sit below the globals a
/// difference that far upstream moves every global address.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// The initializer bytes decode in IR (`tbl`, `single`); the RAM
/// addresses below come from the binary's `--map`, not a second
/// pipeline.
fn assert_ir_bytes(fixture: &str, idx: u8) {
    let (clang, resdir) = driver::clang::pic_clang_from_env();
    let ll_text = driver::clang::compile_to_stdout(
        &clang,
        &resdir,
        std::path::Path::new(fixture),
        &driver::clang::Options {
            includes: vec!["tests/fixtures".to_string()],
            defines: vec![format!("IDX={idx}")],
            ..Default::default()
        },
    );
    {
        let m0 = irparse::parse_ll(&ll_text);
        let gtbl = m0
            .globals
            .iter()
            .find(|g| g.name == "tbl")
            .expect("tbl global");
        assert_eq!(
            gtbl.bytes,
            vec![11, 22, 33, 44, 55, 66, 77, 88],
            "irparse bytes for tbl: {:?}",
            gtbl.bytes
        );
        let gsingle = m0
            .globals
            .iter()
            .find(|g| g.name == "single")
            .expect("single");
        assert_eq!(
            gsingle.bytes,
            vec![99, 101, 103, 105],
            "single bytes {:?}",
            gsingle.bytes
        );
    }
}

fn run_on_device(device_str: &str, idx: u8) {
    assert_ir_bytes("tests/fixtures/named_struct_globals.c", idx);

    let hex_name = format!("tests/fixtures/named_struct_globals_{device_str}_{idx}.hex");
    let map_name = format!("tests/fixtures/named_struct_globals_{device_str}_{idx}.map");
    let output = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            "tests/fixtures/named_struct_globals.c",
            "-o",
            &hex_name,
            "--map",
            &map_name,
            "--device",
            device_str,
            // `idx`'s input rides in as a real initializer (`volatile
            // unsigned char idx = IDX`): __start clears zero-initialized
            // globals before main (epic-cc#561), which would erase a
            // sim-side seed.
            "-D",
        ])
        .arg(format!("IDX={idx}"))
        .output()
        .expect("run driver");
    assert!(
        output.status.success(),
        "driver --device {device_str} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let map = std::fs::read_to_string(&map_name).expect("read map");
    let _ = std::fs::remove_file(&map_name);
    let addr = |n: &str| map_addr(&map, n);

    let hex = std::fs::read_to_string(&hex_name).unwrap();

    if device_str == "p16f877a" {
        let mut p = pic14_sim::Pic14::new(pic14_sim::parse_hex(&hex));
        p.run(200_000);
        assert!(p.halted(), "PIC14 idx={idx} should halt");
        assert_eq!(p.ram()[addr("out0")], 11, "out0 tbl[0][0]");
        assert_eq!(p.ram()[addr("out1")], 22, "out1 tbl[0][1]");
        assert_eq!(p.ram()[addr("out2")], 77, "out2 tbl[1][2]");
        assert_eq!(
            p.ram()[addr("out3")],
            if idx == 0 { 44 } else { 88 },
            "out3 tbl[idx][3]"
        );
        assert_eq!(p.ram()[addr("out4")], 11, "out4 tbl[0].a");
        assert_eq!(p.ram()[addr("out5")], 66, "out5 tbl[1].b");
        assert_eq!(
            p.ram()[addr("out6")],
            if idx == 0 { 33 } else { 77 },
            "out6 tbl[idx].c"
        );
        assert_eq!(
            p.ram()[addr("out7")],
            if idx == 0 { 44 } else { 88 },
            "out7 tbl[idx].d"
        );
        assert_eq!(p.ram()[addr("out_s0")], 99, "single byte0");
        assert_eq!(p.ram()[addr("out_s1")], 101, "single b");
        assert_eq!(p.ram()[addr("out_s2")], 103, "single byte2");
        assert_eq!(p.ram()[addr("out_s3")], 105, "single d");
    } else {
        let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
        p.run(200_000);
        assert!(p.halted(), "PIC18 idx={idx} should halt");
        assert_eq!(p.ram()[addr("out0")], 11, "PIC18 out0");
        assert_eq!(p.ram()[addr("out1")], 22);
        assert_eq!(p.ram()[addr("out2")], 77);
        assert_eq!(p.ram()[addr("out3")], if idx == 0 { 44 } else { 88 });
        assert_eq!(p.ram()[addr("out4")], 11);
        assert_eq!(p.ram()[addr("out5")], 66);
        assert_eq!(p.ram()[addr("out6")], if idx == 0 { 33 } else { 77 });
        assert_eq!(p.ram()[addr("out7")], if idx == 0 { 44 } else { 88 });
        assert_eq!(p.ram()[addr("out_s0")], 99);
        assert_eq!(p.ram()[addr("out_s1")], 101);
        assert_eq!(p.ram()[addr("out_s2")], 103);
        assert_eq!(p.ram()[addr("out_s3")], 105);
    }

    let _ = std::fs::remove_file(&hex_name);
}

#[test]
fn named_struct_globals_pic14() {
    for idx in [0u8, 1] {
        run_on_device("p16f877a", idx);
    }
}

#[test]
fn named_struct_globals_pic18() {
    for idx in [0u8, 1] {
        run_on_device("p18f4550", idx);
    }
}
