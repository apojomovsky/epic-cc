//! A static `memset` cannot serve `llvm.memset` (epic-cc#1453).

use std::process::Command;

fn driver() -> Command {
    Command::new(env!("CARGO_BIN_EXE_epic-cc"))
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
        + &String::from_utf8_lossy(&out.stdout).to_string()
}

fn tmp_c(name: &str, body: &str) -> String {
    let path =
        std::env::temp_dir().join(format!("epic-static-memset-{}-{name}", std::process::id()));
    std::fs::write(&path, body).expect("write temp C file");
    path.display().to_string()
}

fn compile(name: &str, body: &str) -> std::process::Output {
    let src = tmp_c(name, body);
    let out = driver()
        .args([
            src.as_str(),
            "-o",
            "/tmp/epic-static-memset.hex",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    let _ = std::fs::remove_file(&src);
    out
}

// The blob escapes into `consume`, so its zero-initialization stays a
// materialized `llvm.memset` call instead of folding to a constant read.
const ZERO_INIT: &str = r#"
struct Blob { unsigned char b[48]; };
volatile unsigned char sink;
__attribute__((noinline)) void consume(struct Blob *blob) {
    unsigned char sum = 0;
    for (int i = 0; i < 48; i++) sum = (unsigned char)(sum + blob->b[i]);
    sink = sum;
}
int main(void) {
    struct Blob blob = {0};
    blob.b[3] = 7;
    consume(&blob);
    return 0;
}
"#;

#[test]
fn static_memset_zero_init_is_a_named_error_not_an_ice() {
    let body = format!(
        r#"
__attribute__((noinline)) static void *memset(void *s, int c, unsigned long n) {{
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}}
{ZERO_INIT}"#
    );
    let out = compile("static-memset.c", &body);
    assert!(!out.status.success(), "compile should fail");
    assert_eq!(out.status.code(), Some(1));
    let err = stderr_of(&out);
    assert!(err.contains("error:"), "{err}");
    assert!(err.contains("static"), "{err}");
    assert!(err.contains("memset"), "{err}");
    assert!(err.contains("llvm.memset"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
    assert!(!err.contains("unreachable-body"), "{err}");
    assert!(!err.contains("panicked at"), "{err}");
}

#[test]
fn called_static_memset_zero_init_is_a_named_error_not_an_ice() {
    // The explicit call keeps the definition alive, so clang lowers it
    // to `define internal fastcc` with rewritten arguments.
    let body = format!(
        r#"
__attribute__((noinline)) static void *memset(void *s, int c, unsigned long n) {{
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}}
static unsigned char buf[48];
{ZERO_INIT}
"#
    );
    let body = body.replace(
        "    struct Blob blob = {0};",
        "    struct Blob blob = {0};\n    memset(buf, 1, 48);",
    );
    let out = compile("called-static-memset.c", &body);
    assert!(!out.status.success(), "compile should fail");
    assert_eq!(out.status.code(), Some(1));
    let err = stderr_of(&out);
    assert!(err.contains("static memset"), "{err}");
    assert!(err.contains("llvm.memset"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
    assert!(!err.contains("unreachable-body"), "{err}");
}

#[test]
fn called_static_memset_with_dead_intrinsic_still_compiles() {
    // The static body serves its direct calls, and the only intrinsic
    // sits in a never-called function that opt deletes, so no stage
    // ever needs a library memset (epic-cc#1453 review).
    let body = r#"
__attribute__((noinline)) static void *memset(void *s, int c, unsigned long n) {
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}
struct Blob { unsigned char b[48]; };
volatile unsigned char sink;
__attribute__((noinline)) void consume(struct Blob *blob) {
    unsigned char sum = 0;
    for (int i = 0; i < 48; i++) sum = (unsigned char)(sum + blob->b[i]);
    sink = sum;
}
__attribute__((noinline)) void dead_fn(void) {
    struct Blob blob = {0};
    consume(&blob);
}
static unsigned char buf[48];
int main(void) {
    memset(buf, 1, 48);
    sink = buf[47];
    return 0;
}
"#;
    let out = compile("static-memset-dead-intrinsic.c", body);
    let err = stderr_of(&out);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("static memset cannot serve"), "{err}");
}

#[test]
fn static_memset_in_header_is_a_named_error_not_an_ice() {
    let dir = std::env::temp_dir().join(format!("epic-static-memset-hdr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("make temp dir");
    let header = dir.join("helper.h");
    std::fs::write(
        &header,
        r#"
__attribute__((noinline)) static void *memset(void *s, int c, unsigned long n) {
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}
"#,
    )
    .expect("write header");
    let src = dir.join("uses_header.c");
    std::fs::write(&src, format!("#include \"helper.h\"\n{ZERO_INIT}")).expect("write source");
    let out = driver()
        .args([
            src.to_str().unwrap(),
            "-I",
            dir.to_str().unwrap(),
            "-o",
            "/tmp/epic-static-memset.hex",
            "--device",
            "p18f4550",
        ])
        .output()
        .expect("run driver");
    let err = stderr_of(&out);
    assert!(!out.status.success(), "compile should fail: {err}");
    assert!(err.contains("static memset"), "{err}");
    assert!(err.contains("llvm.memset"), "{err}");
    assert!(!err.contains("internal compiler error"), "{err}");
    assert!(!err.contains("panicked at"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn external_memset_still_compiles() {
    let body = r#"
__attribute__((noinline)) void *memset(void *s, int c, unsigned long n) {
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}
volatile unsigned char sink;
static unsigned char buf[48];
int main(void) {
    memset(buf, 0, 48);
    sink = buf[47];
    return 0;
}
"#;
    let out = compile("external-memset.c", body);
    let err = stderr_of(&out);
    assert!(out.status.success(), "{err}");
    assert!(!err.contains("static memset"), "{err}");
}

#[test]
fn static_helper_with_another_name_still_compiles() {
    let body = r#"
__attribute__((noinline)) static void *clear_buf(void *s, int c, unsigned long n) {
    unsigned char *p = (unsigned char *)s;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return s;
}
volatile unsigned char sink;
static unsigned char buf[48];
int main(void) {
    clear_buf(buf, 0, 48);
    sink = buf[47];
    return 0;
}
"#;
    let out = compile("static-helper.c", body);
    let err = stderr_of(&out);
    assert!(out.status.success(), "{err}");
}
