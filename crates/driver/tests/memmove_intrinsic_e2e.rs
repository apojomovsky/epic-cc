//! `__builtin_memmove` lowers to `llvm.memmove`, which must reach the
//! provided or user-defined `memmove` body even though no direct call
//! to it exists before `irparse` rewrites the intrinsic.

use std::process::Command;

fn run_p18(fixture: &str) -> (Vec<u8>, String, bool) {
    let hex_path = std::env::temp_dir().join(format!(
        "memmove_intrinsic_{}_{}.hex",
        std::process::id(),
        std::path::Path::new(fixture)
            .file_stem()
            .unwrap()
            .to_string_lossy()
    ));
    let map_path = hex_path.with_extension("map");
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args(["--device", "p18f4550", "-o"])
        .arg(&hex_path)
        .arg("--map")
        .arg(&map_path)
        .arg(fixture)
        .output()
        .expect("run driver");
    assert!(
        out.status.success(),
        "driver p18f4550 {fixture}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).unwrap();
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&map_path);
    let _ = std::fs::remove_file(&hex_path);
    let mut p = pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    p.run(200_000);
    (p.ram().to_vec(), map, p.halted())
}

fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

fn initial() -> [u8; 16] {
    let mut buf = [0u8; 16];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(7).wrapping_add(3);
    }
    buf
}

fn memmove(buf: &mut [u8; 16], dst: usize, src: usize, n: usize) {
    let tmp: Vec<u8> = buf[src..src + n].to_vec();
    buf[dst..dst + n].copy_from_slice(&tmp);
}

fn check(fixture: &str, moves: &[(usize, usize)]) {
    let (ram, map, halted) = run_p18(fixture);
    let mut expected = initial();
    for &(dst, src) in moves {
        memmove(&mut expected, dst, src, 5);
    }
    let buf = map_addr(&map, "buf");
    assert_eq!(
        &ram[buf..buf + 16],
        &expected[..],
        "{fixture} buf after overlapping memmoves (n=5)"
    );
    let sum = expected.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    assert_eq!(ram[map_addr(&map, "out")], sum, "{fixture} out checksum");
    assert!(halted, "{fixture} must halt");
}

#[test]
fn builtin_memmove_driver_provided_runs_on_p18() {
    check(
        "tests/fixtures/memmove_intrinsic.c",
        &[(2, 5), (6, 4), (0, 0)],
    );
}

#[test]
fn builtin_memmove_user_defined_runs_on_p18() {
    check("tests/fixtures/memmove_intrinsic_user.c", &[(2, 5), (6, 4)]);
}

#[test]
fn builtin_memmove_without_header_or_definition_runs_on_p18() {
    check("tests/fixtures/memmove_intrinsic_bare.c", &[(6, 4), (1, 3)]);
}

#[test]
fn builtin_memmove_with_user_strlen_runs_on_p18() {
    let fixture = "tests/fixtures/memmove_own_strlen.c";
    let (ram, map, halted) = run_p18(fixture);
    let mut expected = initial();
    memmove(&mut expected, 6, 4, 5);
    memmove(&mut expected, 1, 3, 5);
    let buf = map_addr(&map, "buf");
    assert_eq!(
        &ram[buf..buf + 16],
        &expected[..],
        "{fixture} buf after moves"
    );
    let sum = expected.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    assert_eq!(
        ram[map_addr(&map, "out")],
        sum.wrapping_add(0xEE),
        "{fixture} out == checksum plus the user strlen marker"
    );
    assert!(halted, "{fixture} must halt");
}
