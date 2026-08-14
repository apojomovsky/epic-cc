use irparse::parse_ll;

const LL: &str = r#"
@in = dso_local global i8 0, align 1
@out = dso_local global i8 0, align 1
define dso_local void @main() {
  %1 = load volatile i8, ptr @in, align 1
  %2 = add nsw i8 %1, 1
  store volatile i8 %2, ptr @out, align 1
  ret void
}
"#;

#[test]
fn parses_straight_line_ll() {
    let m = parse_ll(LL);
    assert_eq!(m.globals.len(), 2);
    assert_eq!(m.funcs.len(), 1);
    assert_eq!(m.funcs[0].blocks.len(), 1);
    assert_eq!(m.funcs[0].blocks[0].insts.len(), 4);
}
