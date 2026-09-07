//! Phase-2 typed variable table: `parse_debug_vars` reads full `-g`
//! metadata (`DIType`, `DILocalVariable`, `DIGlobalVariable`) and the
//! `#dbg_value`/`#dbg_declare`/`#dbg_assign` body records, while
//! `parse_ll` drops those records without emitting an `Inst`. The
//! fixture mirrors what the pinned clang 20.1.8 at `-O1` emits
//! (observed 2026-09-06), including the fragment/`#dbg_assign` shapes
//! and a lexical-block scope.

use irparse::{parse_debug_vars, parse_ll, DiTypeNode};

const DBG_MODULE: &str = r#"
@g_cnt = dso_local local_unnamed_addr global i16 0, align 2, !dbg !0
@buf = dso_local global [4 x i16] zeroinitializer, align 2, !dbg !13

define dso_local i16 @add(i16 noundef %0, i16 noundef %1) local_unnamed_addr #0 !dbg !25 {
    #dbg_value(i16 %0, !29, !DIExpression(), !31)
    #dbg_value(i16 %1, !30, !DIExpression(), !31)
  %3 = add nsw i16 %1, %0, !dbg !32
  ret i16 %3, !dbg !33
}

define dso_local noundef i16 @main() local_unnamed_addr #1 !dbg !34 {
    #dbg_assign(i1 undef, !37, !DIExpression(), !49, ptr %1, !DIExpression(), !50)
    #dbg_value(i16 1, !41, !DIExpression(DW_OP_LLVM_fragment, 0, 16), !50)
    #dbg_value(ptr null, !43, !DIExpression(), !50)
    #dbg_value(i32 0, !45, !DIExpression(), !50)
    #dbg_value(i16 0, !47, !DIExpression(), !51)
  %2 = load i16, ptr @g_cnt, align 2, !dbg !52
  ret i16 %2, !dbg !58
}

!llvm.dbg.cu = !{!2}
!0 = !DIGlobalVariableExpression(var: !1, expr: !DIExpression())
!1 = distinct !DIGlobalVariable(name: "g_cnt", scope: !2, file: !3, line: 3, type: !11, isLocal: false, isDefinition: true)
!2 = distinct !DICompileUnit(language: DW_LANG_C11, file: !3, producer: "clang version 20.1.8", isOptimized: true, runtimeVersion: 0, emissionKind: FullDebug, enums: !4, globals: !12, nameTableKind: None)
!3 = !DIFile(filename: "t.c", directory: "/w")
!4 = !{!5}
!5 = !DICompositeType(tag: DW_TAG_enumeration_type, name: "Color", file: !3, line: 2, baseType: !6, size: 16, elements: !7)
!6 = !DIBasicType(name: "unsigned int", size: 16, encoding: DW_ATE_unsigned)
!7 = !{!8, !9}
!8 = !DIEnumerator(name: "RED", value: 0)
!9 = !DIEnumerator(name: "GREEN", value: 1)
!11 = !DIBasicType(name: "int", size: 16, encoding: DW_ATE_signed)
!12 = !{!0}
!13 = !DIGlobalVariableExpression(var: !14, expr: !DIExpression())
!14 = distinct !DIGlobalVariable(name: "buf", scope: !2, file: !3, line: 4, type: !38, isLocal: false, isDefinition: true)
!15 = distinct !DICompositeType(tag: DW_TAG_structure_type, name: "P", file: !3, line: 1, size: 32, elements: !16)
!16 = !{!17, !18}
!17 = !DIDerivedType(tag: DW_TAG_member, name: "x", scope: !15, file: !3, line: 1, baseType: !11, size: 16)
!18 = !DIDerivedType(tag: DW_TAG_member, name: "c", scope: !15, file: !3, line: 1, baseType: !19, size: 8, offset: 16)
!19 = !DIBasicType(name: "char", size: 8, encoding: DW_ATE_unsigned_char)
!25 = distinct !DISubprogram(name: "add", scope: !3, file: !3, line: 5, type: !26, scopeLine: 5, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2)
!26 = !DISubroutineType(types: !27)
!27 = !{!11, !11, !11}
!28 = !{!29, !30}
!29 = !DILocalVariable(name: "a", arg: 1, scope: !25, file: !3, line: 5, type: !11)
!30 = !DILocalVariable(name: "b", arg: 2, scope: !25, file: !3, line: 5, type: !11)
!31 = !DILocation(line: 0, scope: !25)
!32 = !DILocation(line: 5, column: 34, scope: !25)
!33 = !DILocation(line: 5, column: 25, scope: !25)
!34 = distinct !DISubprogram(name: "main", scope: !3, file: !3, line: 6, type: !35, scopeLine: 6, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2)
!36 = !{!37, !41, !43, !45, !47}
!37 = !DILocalVariable(name: "buf", scope: !34, file: !3, line: 7, type: !38)
!38 = !DICompositeType(tag: DW_TAG_array_type, baseType: !11, size: 64, elements: !39)
!39 = !{!40}
!40 = !DISubrange(count: 4)
!41 = !DILocalVariable(name: "p", scope: !34, file: !3, line: 8, type: !15)
!43 = !DILocalVariable(name: "cp", scope: !34, file: !3, line: 10, type: !44)
!44 = !DIDerivedType(tag: DW_TAG_pointer_type, baseType: !19, size: 16)
!45 = !DILocalVariable(name: "total", scope: !34, file: !3, line: 11, type: !46)
!46 = !DIBasicType(name: "long", size: 32, encoding: DW_ATE_signed)
!47 = !DILocalVariable(name: "i", scope: !48, file: !3, line: 12, type: !11)
!48 = distinct !DILexicalBlock(scope: !34, file: !3, line: 12, column: 3)
!49 = distinct !DIAssignID()
!50 = !DILocation(line: 0, scope: !34)
!51 = !DILocation(line: 0, scope: !48)
!52 = !DILocation(line: 13, column: 9, scope: !34)
!58 = !DILocation(line: 15, column: 3, scope: !34)
"#;

fn local_named<'a>(v: &'a irparse::DebugVars, name: &str, func: &str) -> &'a irparse::DiVar {
    v.locals
        .iter()
        .find(|d| d.name == name && d.func.as_deref() == Some(func))
        .unwrap_or_else(|| panic!("no local {name} in {func}: {:?}", v.locals))
}

#[test]
fn dbg_records_emit_no_inst() {
    let m = parse_ll(DBG_MODULE);
    assert_eq!(m.funcs.len(), 2);
    let add = m.funcs.iter().find(|f| f.name == "add").unwrap();
    // Only `%3 = add` and `ret` survive; the two #dbg_value lines vanish.
    let n: usize = add.blocks.iter().map(|b| b.insts.len()).sum();
    assert_eq!(n, 2, "dbg records must not become Insts: {:?}", add.blocks);
    let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
    let n: usize = main.blocks.iter().map(|b| b.insts.len()).sum();
    assert_eq!(
        n, 2,
        "dbg_assign must not become an Inst: {:?}",
        main.blocks
    );
}

#[test]
fn locals_resolve_names_scopes_and_ssa_operands() {
    let v = parse_debug_vars(DBG_MODULE);
    // Args join through their SSA operands (`i16 %0` / `i16 %1`).
    let a = local_named(&v, "a", "add");
    assert_eq!((a.ssa.as_deref(), a.arg), (Some("0"), Some(1)));
    let b = local_named(&v, "b", "add");
    assert_eq!(b.ssa.as_deref(), Some("1"));
    // The array's storage comes from #dbg_assign's address operand.
    assert_eq!(local_named(&v, "buf", "main").ssa.as_deref(), Some("1"));
    // A nested lexical block still resolves to its function; its loop
    // counter was constant-folded at -O1, so no SSA operand survives.
    assert_eq!(local_named(&v, "i", "main").func.as_deref(), Some("main"));
    assert_eq!(local_named(&v, "i", "main").ssa, None);
    // Constant operands (`ptr null`, `i32 0`, fragment values) map no SSA:
    // the variable is optimized away, omitted from the address join later.
    assert_eq!(local_named(&v, "cp", "main").ssa, None);
    assert_eq!(local_named(&v, "total", "main").ssa, None);
    assert_eq!(local_named(&v, "p", "main").ssa, None);
}

#[test]
fn type_nodes_cover_scalars_aggregates_pointers() {
    let v = parse_debug_vars(DBG_MODULE);
    let ty = |name: &str, func: &str| {
        let d = local_named(&v, name, func);
        v.type_string(d.ty)
    };
    assert_eq!(ty("a", "add"), "int");
    assert_eq!(ty("cp", "main"), "char*");
    assert_eq!(ty("total", "main"), "long");
    assert_eq!(ty("buf", "main"), "int[4]");
    assert_eq!(ty("p", "main"), "struct P");
    // Struct members carry byte offsets; `char c` sits at offset 2.
    let p = v.locals.iter().find(|d| d.name == "p").unwrap();
    let Some(DiTypeNode::Struct { members, .. }) = v.types.get(&p.ty) else {
        panic!("p must be a struct");
    };
    let c = members.iter().find_map(|m| match v.types.get(m) {
        Some(DiTypeNode::Member { name, offset, .. }) if name.as_deref() == Some("c") => {
            offset.clone()
        }
        _ => None,
    });
    assert_eq!(c, Some(2));
}

#[test]
fn globals_resolve_names_and_types() {
    let v = parse_debug_vars(DBG_MODULE);
    let names: Vec<&str> = v.globals.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["buf", "g_cnt"]);
    let cnt = v.globals.iter().find(|g| g.name == "g_cnt").unwrap();
    assert_eq!(v.type_string(cnt.ty), "int");
    let buf = v.globals.iter().find(|g| g.name == "buf").unwrap();
    assert_eq!(v.type_string(buf.ty), "int[4]");
}
