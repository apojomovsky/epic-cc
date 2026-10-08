//! epic-cc#909: an unsized `extern` array with no definition is a missing
//! definition, a user error. The merged IR carries a bare `external`
//! declaration; the driver must report it as an undefined symbol naming
//! the table, not crash in irparse.

use std::process::Command;

fn run_driver(name: &str, src: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("epic-cc-909-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let c_path = dir.join(format!("{name}.c"));
    std::fs::write(&c_path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_epic-cc"))
        .args([
            c_path.to_str().unwrap(),
            "-o",
            dir.join("out.hex").to_str().unwrap(),
            "--device",
            "p16f877a",
        ])
        .output()
        .expect("run driver");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.success(), stderr)
}

#[test]
fn unsized_extern_array_without_definition_names_the_table() {
    let (ok, stderr) = run_driver(
        "irq_table",
        "typedef struct { unsigned char vec; unsigned char pri; } irq_desc_t;\n\
         extern const irq_desc_t irq_table[];\n\
         int main(void) { return irq_table[0].vec; }\n",
    );
    assert!(
        !ok,
        "a missing table definition must be rejected, not compiled"
    );
    assert!(
        stderr.contains("undefined symbols: irq_table"),
        "diagnostic must name the table:\n{stderr}"
    );
}

#[test]
fn extern_scalar_without_definition_names_the_symbol() {
    let (ok, stderr) = run_driver(
        "missing_scalar",
        "extern int missing_scalar;\nint main(void) { return missing_scalar; }\n",
    );
    assert!(
        !ok,
        "a missing scalar definition must be rejected, not compiled"
    );
    assert!(
        stderr.contains("undefined symbols: missing_scalar"),
        "diagnostic must name the symbol:\n{stderr}"
    );
}
