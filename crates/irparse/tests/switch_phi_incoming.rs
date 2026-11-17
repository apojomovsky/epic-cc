//! epic-cc#1417: the compare-chain expansion of an LLVM `switch` changes
//! a case target's predecessor, so the target's phi must name the chain
//! block that now branches to it, not the switch block it came from.
//!
//! The backends key their per-edge phi copies on the real predecessor,
//! so a stale label makes the lookup miss and the phi's init copy (a
//! loop counter's zero init) is dropped: the slot keeps whatever the
//! previous writer left there.

use ir::Inst;
use irparse::parse_ll;

/// A switch whose second case is a loop header carrying a counter phi.
/// clang's shape: the case target's phi names the switch block.
const SWITCH_LOOP_LL: &str = r#"define void @handle(i8 noundef zeroext %0) #0 {
  switch i8 %0, label %9 [
    i8 32, label %4
    i8 33, label %5
  ]

4:                                                ; preds = %1
  br label %9

5:                                                ; preds = %1, %5
  %2 = phi i16 [ 0, %1 ], [ %3, %5 ]
  %3 = add nuw nsw i16 %2, 1
  %6 = icmp eq i16 %3, 7
  br i1 %6, label %9, label %5

9:                                                ; preds = %5, %4, %1
  ret void
}

attributes #0 = { noinline nounwind }"#;

/// The case target's phi names the chain block that branches to it, so
/// the backend finds the edge's copy.
#[test]
fn case_target_phi_names_the_chain_block() {
    let m = parse_ll(SWITCH_LOOP_LL);
    let handle = m.funcs.iter().find(|f| f.name == "handle").unwrap();
    let target = handle
        .blocks
        .iter()
        .find(|b| b.label == "5")
        .expect("the case target block");
    let phi = target
        .insts
        .iter()
        .find_map(|i| match i {
            Inst::Phi(p) => Some(p),
            _ => None,
        })
        .expect("the case target carries a phi");
    let preds: Vec<&str> = phi.incoming.iter().map(|(_, f)| f.as_str()).collect();
    // The chain block is the fresh `__switchN` the expansion made; the
    // backedge incoming still names the target itself.
    assert!(
        preds.iter().any(|p| p.starts_with("__switch")),
        "the phi still names the pre-expansion switch block: {preds:?}"
    );
    assert!(
        !preds.contains(&"1"),
        "the phi still names the pre-expansion switch block: {preds:?}"
    );
    assert!(
        preds.contains(&"5"),
        "the phi lost its backedge incoming: {preds:?}"
    );
}

/// The chain's blocks branch to the case targets, so the phi renamed
/// above names a real predecessor.
#[test]
fn renamed_phi_names_a_real_predecessor() {
    let m = parse_ll(SWITCH_LOOP_LL);
    let handle = m.funcs.iter().find(|f| f.name == "handle").unwrap();
    let target = handle
        .blocks
        .iter()
        .find(|b| b.label == "5")
        .expect("the case target block");
    let phi = target
        .insts
        .iter()
        .find_map(|i| match i {
            Inst::Phi(p) => Some(p),
            _ => None,
        })
        .expect("the case target carries a phi");
    // Every incoming names a block that really branches to the target.
    for (_, from) in &phi.incoming {
        let branches_here = handle.blocks.iter().any(|b| {
            b.label == *from
                && matches!(
                    b.insts.last(),
                    Some(Inst::Br(br)) if br.target == "5"
                )
                || matches!(
                    b.insts.last(),
                    Some(Inst::BrCond(bc)) if bc.t == "5" || bc.f == "5"
                )
        });
        assert!(branches_here, "the incoming {from} is not a predecessor");
    }
}
