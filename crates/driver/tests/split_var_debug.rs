//! A local SROA split into separately colored allocas has no one
//! address (epic-cc#1242): `--var-table` and the sidecar DWARF must
//! describe each piece where the allocator put it. The layout here is
//! hand-built so the pieces are non-adjacent whatever the allocator
//! happens to do today.

use alloc::AllocLayout;
use gimli::{constants, AttributeValue, EndianSlice, LittleEndian};
use object::{Object, ObjectSection};

use driver::report::var_table_text;
use driver::sidecar::encode;

/// `volatile int arr[4]` after SROA: bytes 0..2 in `%1`, 2..4 in `%2`,
/// the unused tail with no storage.
const SPLIT_ARR: &str = r#"
define dso_local i16 @main() local_unnamed_addr #1 !dbg !30 {
  %1 = alloca i16, align 2, !DIAssignID !39
    #dbg_assign(i1 undef, !34, !DIExpression(DW_OP_LLVM_fragment, 0, 16), !39, ptr %1, !DIExpression(), !40)
  %2 = alloca i16, align 2, !DIAssignID !41
    #dbg_assign(i1 undef, !34, !DIExpression(DW_OP_LLVM_fragment, 16, 16), !41, ptr %2, !DIExpression(), !40)
    #dbg_assign(i1 undef, !34, !DIExpression(DW_OP_LLVM_fragment, 32, 32), !42, ptr undef, !DIExpression(), !40)
  ret i16 0
}
!30 = distinct !DISubprogram(name: "main", scope: !3, file: !3, line: 15, type: !31, scopeLine: 15, spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !2)
!34 = !DILocalVariable(name: "arr", scope: !30, file: !3, line: 16, type: !35)
!35 = !DICompositeType(tag: DW_TAG_array_type, baseType: !36, size: 64, elements: !37)
!36 = !DIDerivedType(tag: DW_TAG_volatile_type, baseType: !6)
!37 = !{!38}
!38 = !DISubrange(count: 4)
!6 = !DIBasicType(name: "int", size: 16, encoding: DW_ATE_signed)
"#;

fn layout(first: u16, second: u16) -> AllocLayout {
    let mut l = AllocLayout::default();
    l.locals.insert("main::1".to_string(), first);
    l.locals.insert("main::2".to_string(), second);
    l
}

#[test]
fn var_table_reports_each_piece_where_it_landed() {
    let vars = irparse::parse_debug_vars(SPLIT_ARR);
    let dev = device::resolve("p16f877a").expect("p16f877a");
    let t = var_table_text(dev, &layout(0x2B, 0x2F), &vars);
    assert!(
        t.contains("local main::arr 0x2B int[4] ; bytes 0+2\n")
            && t.contains("local main::arr 0x2F int[4] ; bytes 2+2\n"),
        "{t}"
    );
    // Adjacent pieces still read as the one object they used to be.
    let t = var_table_text(dev, &layout(0x2B, 0x2D), &vars);
    assert!(t.contains("local main::arr 0x2B int[4]\n"), "{t}");
    assert!(!t.contains("bytes"), "{t}");
}

/// The `DW_AT_location` expression bytes of the variable DIE `name`.
fn location_of(elf: &[u8], name: &str) -> Vec<u8> {
    let obj = object::File::parse(elf).expect("sidecar ELF");
    let dwarf = gimli::Dwarf::load(|id| {
        let data = obj
            .section_by_name(id.name())
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok::<_, gimli::Error>(EndianSlice::new(data, LittleEndian))
    })
    .expect("load DWARF");
    let mut units = dwarf.units();
    while let Some(header) = units.next().expect("unit header") {
        let unit = dwarf.unit(header).expect("unit");
        let mut entries = unit.entries();
        while let Some((_, e)) = entries.next_dfs().expect("entry") {
            if e.tag() != constants::DW_TAG_variable {
                continue;
            }
            let n = e.attr_value(constants::DW_AT_name).unwrap().unwrap();
            let n = dwarf.attr_string(&unit, n).unwrap();
            if n.to_string_lossy() != name {
                continue;
            }
            let Some(AttributeValue::Exprloc(expr)) =
                e.attr_value(constants::DW_AT_location).unwrap()
            else {
                panic!("{name} has no exprloc");
            };
            return expr.0.slice().to_vec();
        }
    }
    panic!("no variable {name} in the sidecar");
}

#[test]
fn sidecar_describes_a_split_array_as_pieces() {
    let vars = irparse::parse_debug_vars(SPLIT_ARR);
    let loc = location_of(&encode(&[], &layout(0x2B, 0x2F), &vars), "arr");
    // addr 0x2B; piece 2; addr 0x2F; piece 2; an empty piece for the
    // 4 bytes with no storage, so gdb prints them as optimized out.
    let (addr, piece) = (constants::DW_OP_addr.0, constants::DW_OP_piece.0);
    assert_eq!(
        loc,
        vec![addr, 0x2B, 0, 0, 0, piece, 2, addr, 0x2F, 0, 0, 0, piece, 2, piece, 4]
    );
}
