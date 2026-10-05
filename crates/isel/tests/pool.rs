//! `build_pool` unit tests (epic-cc#815): ordering, dedup, packing,
//! ref rebasing, and the skip rules, over hand-built modules.

use std::collections::{HashMap, HashSet};

fn cg(name: &str, bytes: &[u8], refs: Vec<(usize, String, u16)>) -> ir::Global {
    ir::Global {
        name: name.to_string(),
        ty: ir::Ty::I8,
        is_const: true,
        size: bytes.len() as u16,
        bytes: bytes.to_vec(),
        refs,
        addr: None,
    }
}

fn pool_of(members: Vec<ir::Global>) -> isel::ConstPool {
    let set: HashSet<String> = members.iter().map(|g| g.name.clone()).collect();
    let m = ir::Module {
        globals: members,
        funcs: Vec::new(),
        module_asm: Vec::new(),
    };
    isel::build_pool(&m, &set)
}

#[test]
fn pool_concatenates_members_in_name_order() {
    let pool = pool_of(vec![cg("b", b"BB", vec![]), cg("a", b"A\0", vec![])]);
    assert_eq!(pool.chunks.len(), 1);
    assert_eq!(pool.chunks[0].name, "__const_pool");
    // "a" sorts first, NUL included: member bytes verbatim.
    assert_eq!(pool.chunks[0].bytes, b"A\0BB");
    assert!(pool.chunks[0].refs.is_empty());
}

#[test]
fn pool_dedups_byte_identical_members() {
    let pool = pool_of(vec![
        cg("a", b"same", vec![]),
        cg("b", b"same", vec![]),
        cg("c", b"diff", vec![]),
    ]);
    assert_eq!(pool.chunks.len(), 1);
    assert_eq!(pool.chunks[0].bytes, b"samediff");
}

#[test]
fn pool_refs_rebase_to_chunk_offsets() {
    // Two members with refs to different targets at member-local
    // offsets: the pool keeps targets and addends, rebased.
    let pool = pool_of(vec![
        cg("a", &[0, 0, 0], vec![(1, "f".to_string(), 0)]),
        cg("b", &[0, 0], vec![(0, "g".to_string(), 7)]),
    ]);
    assert_eq!(pool.chunks.len(), 1);
    assert_eq!(
        pool.chunks[0].refs,
        vec![(1, "f".to_string(), 0), (3, "g".to_string(), 7)]
    );
}

#[test]
fn pool_dedup_distinguishes_refs() {
    // Same bytes but different refs are different content: no merge.
    let pool = pool_of(vec![
        cg("a", &[0, 0], vec![(0, "f".to_string(), 0)]),
        cg("b", &[0, 0], vec![(0, "g".to_string(), 0)]),
    ]);
    assert_eq!(pool.chunks[0].bytes, vec![0, 0, 0, 0]);
}

#[test]
fn pool_packs_whole_members_and_splits_at_255() {
    // 200 + 100 cannot share a 255-byte chunk; the 100-byte member
    // starts chunk 1 whole, never straddling.
    let big: Vec<u8> = (0..200u8).collect();
    let pool = pool_of(vec![cg("a", &big, vec![]), cg("b", &[9u8; 100], vec![])]);
    assert_eq!(pool.chunks.len(), 2);
    assert_eq!(pool.chunks[0].name, "__const_pool");
    assert_eq!(pool.chunks[1].name, "__const_pool_1");
    assert_eq!(pool.chunks[0].bytes.len(), 200);
    assert_eq!(pool.chunks[1].bytes, [9u8; 100]);
}

#[test]
fn pool_skips_non_flash_candidates() {
    // Empty, oversized, mapped, and non-const members never pool, even
    // when the candidate set names them.
    let mut pinned = cg("pinned", b"zz", vec![]);
    pinned.addr = Some(0x40);
    let mut ram = cg("ram", b"zz", vec![]);
    ram.is_const = false;
    let pool = pool_of(vec![
        cg("empty", b"", vec![]),
        cg("huge", &[1u8; 256], vec![]),
        pinned,
        ram,
        cg("ok", b"q", vec![]),
    ]);
    assert_eq!(pool.chunks.len(), 1);
    assert_eq!(pool.chunks[0].bytes, b"q");
}

#[test]
fn pool_ignores_unknown_names() {
    let pool = pool_of(vec![cg("a", b"A", vec![])]);
    let m = ir::Module {
        globals: vec![cg("a", b"A", vec![])],
        funcs: Vec::new(),
        module_asm: Vec::new(),
    };
    let mut set = HashSet::new();
    set.insert("missing".to_string());
    let pool2 = isel::build_pool(&m, &set);
    assert!(pool2.chunks.is_empty());
    assert_eq!(pool.chunks.len(), 1);
}

#[test]
fn pool_chunk_refs_emit_rebased_through_select() {
    // A ref-carrying pool chunk through the real emitter: the pool
    // RETLW run must carry the rebased ref bytes, proving the
    // synthesized-global path does not drop or misplace them.
    let ram = ir::Global {
        name: "r".to_string(),
        ty: ir::Ty::I8,
        is_const: false,
        size: 1,
        bytes: vec![0],
        refs: Vec::new(),
        addr: None,
    };
    let mut m = ir::parse("fn main(void) ()\n  block 0:\n    ret void\n");
    m.globals = vec![
        ram,
        cg("c1", b"AB", vec![]),
        cg("c2", &[0xFF, 0xEE, 0], vec![(2, "r".to_string(), 0)]),
    ];
    let set: HashSet<String> = ["c1".to_string(), "c2".to_string()].into_iter().collect();
    let pool = isel::build_pool(&m, &set);
    let mut addrs = HashMap::new();
    addrs.insert("r".to_string(), 0x30);
    let asm = isel::select_with_locs(&device::PIC16F877A, &m, &addrs, &HashSet::new(), &pool).0;
    let run: Vec<&str> = asm
        .lines()
        .skip_while(|l| l.trim() != "__const_pool:")
        .skip(1)
        .take_while(|l| l.trim().starts_with("RETLW"))
        .collect();
    assert_eq!(
        run,
        vec![
            "    RETLW 0x41",
            "    RETLW 0x42",
            "    RETLW 0xFF",
            "    RETLW 0xEE",
            "    RETLW 0x30",
        ],
        "pool chunk must hold c1 plus c2 with the ref rebased to +4:\n{asm}"
    );
    assert!(
        asm.lines().any(|l| l.trim() == "__read___const_pool:"),
        "pool reader entry expected:\n{asm}"
    );
}

#[test]
fn pool_resolve_maps_members_to_chunk_offsets() {
    // epic-cc#816: every member (duplicates alias the first
    // occurrence) resolves to chunk label plus byte offset for the
    // address rewrite; outsiders and the empty pool resolve to None.
    let pool = pool_of(vec![
        cg("b", b"BB", vec![]),
        cg("a", b"A\0", vec![]),
        cg("dup", b"A\0", vec![]),
    ]);
    assert_eq!(pool.resolve("a"), Some(("__const_pool", 0)));
    assert_eq!(pool.resolve("b"), Some(("__const_pool", 2)));
    assert_eq!(pool.resolve("dup"), Some(("__const_pool", 0)));
    assert_eq!(pool.resolve("missing"), None);
    assert_eq!(pool_of(vec![]).resolve("a"), None);
}

#[test]
fn pooled_const_call_arg_materializes_pool_address() {
    // epic-cc#816: a gated const (no RAM copy under pooling) passes
    // its pool address as a direct call arg instead of panicking on
    // the missing copy.
    let mut m = ir::parse(
        "const s i8\n\
         fn main(void) ()\n\
           block entry:\n\
             call void @f(@s)\n\
             ret void\n\
         fn f(void) (p=ptr)\n\
           block entry:\n\
             ret void\n",
    );
    m.globals[0].bytes = b"s\0".to_vec();
    m.globals[0].size = 2;
    let layout = alloc::allocate_with_pool(&device::PIC16F877A, &m, "edge main f\n", true);
    assert!(
        !layout.globals.contains_key("s"),
        "pooled direct-only const keeps no RAM copy"
    );
    let pool = isel::build_pool(&m, &layout.address_taken_consts);
    let mut addrs = layout.globals.clone();
    addrs.extend(layout.locals.clone());
    let asm = isel::select_with_locs(
        &device::PIC16F877A,
        &m,
        &addrs,
        &layout.staged_consts,
        &pool,
    )
    .0;
    assert!(
        asm.contains("MOVLW LOW(__const_pool)") && asm.contains("MOVLW HIGH(__const_pool)"),
        "direct const call arg must carry the pool address:\n{asm}"
    );
}

#[test]
fn pooled_const_select_arms_materialize_pool_addresses() {
    // epic-cc#816: the select-slot materialization resolves pooled
    // consts absent from the map to chunk addresses, for directly
    // named arms and clean GEP-derived arms alike. Production alloc
    // never sends select arms here copyless (they keep their copies);
    // this pins the emission contract for the hand-mapped path.
    let mut m = ir::parse(
        "const a i8\n\
         const b i8\n\
         fn main(void) ()\n\
           block entry:\n\
             %c = icmp eq i8 1, 1\n\
             %g = gep @b +0\n\
             %s = select i1 %c, ptr %g, ptr @a\n\
             %v = load i8 %s\n\
             ret void\n",
    );
    m.globals[0].bytes = b"a\0".to_vec();
    m.globals[0].size = 2;
    m.globals[1].bytes = b"b\0".to_vec();
    m.globals[1].size = 2;
    let set: HashSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
    let pool = isel::build_pool(&m, &set);
    let mut addrs = HashMap::new();
    addrs.insert("main::c".to_string(), 0x20);
    addrs.insert("main::s".to_string(), 0x21);
    addrs.insert("main::v".to_string(), 0x23);
    let asm = isel::select_with_locs(&device::PIC16F877A, &m, &addrs, &HashSet::new(), &pool).0;
    assert!(
        asm.contains("MOVLW LOW(__const_pool)") && asm.contains("MOVLW LOW(__const_pool+2)"),
        "both select arms must carry pool addresses:\n{asm}"
    );
}
