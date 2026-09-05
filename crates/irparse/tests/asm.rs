use ir::Inst;
use irparse::parse_ll;

#[test]
fn module_asm() {
    let ll = r#"module asm "global_blob: nop""#;
    let m = parse_ll(ll);
    assert_eq!(m.module_asm, vec!["global_blob: nop"]);
}

#[test]
fn naked() {
    let ll = r#"define dso_local void @bar() #0 {
  tail call void asm sideeffect "return", ""() #1
  unreachable
}
attributes #0 = { naked noinline nounwind }"#;
    let m = parse_ll(ll);
    let bar = m.funcs.iter().find(|f| f.name == "bar").unwrap();
    assert!(bar.naked);
    // naked body should contain one Asm, no Ret
    assert!(bar.blocks[0]
        .insts
        .iter()
        .any(|i| matches!(i, Inst::Asm(_))));
    assert!(!bar.blocks[0]
        .insts
        .iter()
        .any(|i| matches!(i, Inst::Ret(_, _))));
}

#[test]
fn opaque_asm_no_operands() {
    let ll = r#"define void @foo() { tail call void asm sideeffect "bcf INTCON, 7", ""() #0
 ret void }"#;
    // use multiline to avoid single-line parsing edge; but also test single-line
    let m = parse_ll(ll);
    let foo = m.funcs.iter().find(|f| f.name == "foo").unwrap();
    assert!(
        matches!(&foo.blocks[0].insts[0], ir::Inst::Asm(a) if a.template=="bcf INTCON, 7" && !a.clobbers_memory)
    );
}

#[test]
fn opaque_asm_single_line() {
    let ll =
        r#"define void @foo() { tail call void asm sideeffect "bcf INTCON, 7", ""() #0 ret void }"#;
    let m = parse_ll(ll);
    let foo = m.funcs.iter().find(|f| f.name == "foo").unwrap();
    assert!(
        matches!(&foo.blocks[0].insts[0], ir::Inst::Asm(a) if a.template=="bcf INTCON, 7" && !a.clobbers_memory)
    );
}

#[test]
fn clobbers_memory_flag() {
    let ll =
        r#"define void @foo() { tail call void asm sideeffect "nop", "~{memory}"() ret void }"#;
    let m = parse_ll(ll);
    assert!(matches!(&m.funcs[0].blocks[0].insts[0], ir::Inst::Asm(a) if a.clobbers_memory));
}

#[test]
#[should_panic(expected = "register constraints are not supported")]
fn rejects_register_constraint() {
    let ll = r#"define void @foo() { %1 = tail call i8 asm sideeffect "movwf $0", "=r,0"(i8 1) ret void }"#;
    parse_ll(ll);
}

#[test]
fn accepts_memory_operands() {
    let ll = r#"define void @foo() { tail call void asm sideeffect "movf $1, w", "=*m,*m,*m"(ptr @t, ptr @y, ptr @t) ret void }"#;
    let m = parse_ll(ll);
    let foo = &m.funcs[0].blocks[0].insts[0];
    if let ir::Inst::Asm(a) = foo {
        assert_eq!(a.template, "movf $1, w");
        assert_eq!(a.operands.len(), 3);
        assert_eq!(a.operands[0].constraint, "=*m");
        assert_eq!(a.operands[0].ptr, "@t");
        assert_eq!(a.operands[1].ptr, "@y");
    } else {
        panic!("expected Asm");
    }
}

#[test]
fn module_asm_with_embedded_newline() {
    // single module asm line with embedded \0A should split into two entries
    let ll = r#"module asm "foo\0Abar"
define void @foo() { ret void }"#;
    let m = parse_ll(ll);
    assert_eq!(m.module_asm, vec!["foo", "bar"]);
}

#[test]
fn module_asm_multiple_lines() {
    let ll = r#"module asm "foo"
module asm "bar"
define void @foo() { ret void }"#;
    let m = parse_ll(ll);
    assert_eq!(m.module_asm, vec!["foo", "bar"]);
}

#[test]
fn unescape_template() {
    // template with \\ and \22 and \0A decoding
    let ll = r#"define void @foo() { tail call void asm sideeffect "a\\b\0Ac", ""() ret void }"#;
    let m = parse_ll(ll);
    let foo = &m.funcs[0].blocks[0].insts[0];
    if let Inst::Asm(a) = &*foo {
        assert_eq!(a.template, "a\\b\nc");
    } else {
        panic!("expected Asm");
    }
}

#[test]
fn sanitize_leaves_asm_string_content_untouched() {
    // sanitize_symbols should not rewrite `@` inside quoted asm strings
    let ll = r#"module asm "x @y"
define void @foo() { tail call void asm sideeffect "ld @x", ""() ret void }"#;
    let sanitized = irparse::sanitize_symbols(ll);
    // The `@y` inside module asm string and `@x` inside asm template should remain
    assert!(sanitized.contains("\"x @y\""));
    assert!(sanitized.contains("\"ld @x\""));
    // but a global symbol `@my.sym` outside strings should be sanitized
    let ll2 = "@my.sym = global i8 0\ndefine void @foo() { ret void }";
    let s2 = irparse::sanitize_symbols(ll2);
    assert!(s2.contains("@my_sym"));
}
