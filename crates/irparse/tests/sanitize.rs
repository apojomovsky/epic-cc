use irparse::sanitize_symbols;

#[test]
fn rewrites_dots_in_symbol_names() {
    let ll = "@scratch.4 = internal global i8 0\ndefine i8 @helper.3(i8 %0) {\n";
    let out = sanitize_symbols(ll);
    assert!(out.contains("@scratch_4 = internal global i8 0"));
    assert!(out.contains("define i8 @helper_3(i8 %0)"));
}

#[test]
fn leaves_undotted_symbols_alone() {
    let ll = "define void @main() {\n  call void @helper()\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn leaves_llvm_intrinsics_alone() {
    // irparse matches these by prefix (`llvm.memcpy.p0.p0`) and they never
    // become assembler labels, so their dots must survive.
    let ll = "  call void @llvm.memcpy.p0.p0.i16(ptr %1, ptr %2, i16 4, i1 false)\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn leaves_registers_and_metadata_alone() {
    // `%` registers are function-local and never collide across modules;
    // `!` metadata and float literals both contain dots that are not symbols.
    let ll = "  %1 = fadd float %0, 1.5, !tbaa !2\n";
    assert_eq!(sanitize_symbols(ll), ll);
}

#[test]
fn does_not_rewrite_inside_string_constants() {
    // A C string literal reaching the .ll as `c"..."` can contain an @ and a
    // dot; rewriting inside it would corrupt program data.
    let ll = "@s = private constant [14 x i8] c\"user@host.com\\00\"\n";
    let out = sanitize_symbols(ll);
    assert!(
        out.contains("c\"user@host.com\\00\""),
        "string constant was rewritten: {out}"
    );
    assert!(out.starts_with("@s = "));
}

#[test]
#[should_panic(expected = "sanitize to @helper_3")]
fn panics_when_two_symbols_collide_after_sanitizing() {
    let ll = "define void @helper.3() {\ndefine void @helper_3() {\n";
    sanitize_symbols(ll);
}
