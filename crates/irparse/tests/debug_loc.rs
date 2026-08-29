use ir::{Inst, SrcLoc};
use irparse::parse_ll;

// A minimal module carrying exactly the line-table metadata clang adds
// under `-gline-tables-only`: a DIFile per file, a DISubprogram per
// definition, a DILocation per instruction.
const DBG_MODULE: &str = r#"
define dso_local i16 @main() local_unnamed_addr #0 !dbg !6 {
  %1 = tail call i16 @helper() #1, !dbg !9
  ret i16 %1, !dbg !10
}

declare i16 @helper() local_unnamed_addr #1

!llvm.dbg.cu = !{!0}

!0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, producer: "clang version 20.1.8", isOptimized: true, runtimeVersion: 0, emissionKind: LineTablesOnly, splitDebugInlining: false, nameTableKind: None)
!1 = !DIFile(filename: "t.c", directory: "/x")
!2 = !{i32 7, !"Dwarf Version", i32 5}
!3 = !{i32 2, !"Debug Info Version", i32 3}
!4 = !{i32 1, !"wchar_size", i32 2}
!5 = !{!"clang version 20.1.8"}
!6 = distinct !DISubprogram(name: "main", scope: !7, file: !7, line: 3, type: !8, scopeLine: 3, flags: DIFlagPrototyped | DIFlagAllCallsDescribed, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !0)
!7 = !DIFile(filename: "t.c", directory: "/x")
!8 = !DISubroutineType(types: !{})
!9 = !DILocation(line: 4, column: 3, scope: !6)
!10 = !DILocation(line: 5, column: 1, scope: !6)
"#;

#[test]
fn call_carries_its_source_location() {
    let m = parse_ll(DBG_MODULE);
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    let call = main.blocks[0]
        .insts
        .iter()
        .find_map(|i| match i {
            Inst::Call(c) if c.func == "helper" => Some(c.loc.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        call,
        Some(SrcLoc {
            file: "t.c".to_string(),
            line: 4,
            col: 3
        })
    );
}

#[test]
fn call_without_metadata_has_no_location() {
    let m = parse_ll(
        "define dso_local i16 @main() local_unnamed_addr #0 {\n  %1 = call i16 @helper()\n  ret i16 %1\n}\n\ndeclare i16 @helper() local_unnamed_addr #1\n",
    );
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    for inst in &main.blocks[0].insts {
        if let Inst::Call(c) = inst {
            assert_eq!(c.loc, None);
        }
    }
}

#[test]
#[should_panic(expected = "t.c:4:3: SPIKE: unsupported type \"double\"")]
fn instruction_panic_names_the_c_line() {
    // The panic fires while the alloca line is parsed; the location is
    // the line's own DILocation, not a Rust call site.
    let src = DBG_MODULE.replace(
        "%1 = tail call i16 @helper() #1, !dbg !9",
        "%1 = alloca double, align 8, !dbg !9",
    );
    let _ = parse_ll(&src);
}

#[test]
#[should_panic(expected = "t.c:2:1: SPIKE: unsupported type \"double\"")]
fn define_panic_names_the_function_site() {
    // The define line's `!dbg` names its subprogram, which has a line but
    // no column; col 1 stands for the function's opening line.
    let src = r#"
define dso_local double @get() local_unnamed_addr #0 !dbg !6 {
  ret double 1.500000e+00, !dbg !9
}

!0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !7, producer: "clang version 20.1.8", isOptimized: true, runtimeVersion: 0, emissionKind: LineTablesOnly, unit: !0)
!6 = distinct !DISubprogram(name: "get", scope: !7, file: !7, line: 2, type: !8, scopeLine: 2, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !0)
!7 = !DIFile(filename: "t.c", directory: "/x")
!8 = !DISubroutineType(types: !{})
!9 = !DILocation(line: 2, column: 25, scope: !6)
"#;
    let _ = parse_ll(src);
}
