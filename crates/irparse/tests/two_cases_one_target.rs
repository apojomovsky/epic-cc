//! epic-cc#1417: a switch with two cases reaching the same target gives
//! that target two predecessors, so its phi needs an incoming for each.
//! LLVM's phi has one entry per predecessor edge, so the switch block
//! is named twice.

use ir::Inst;
use irparse::parse_ll;

const TWO_CASES_LL: &str = r#"define void @handle(i8 noundef zeroext %0) #0 {
  switch i8 %0, label %9 [
    i8 32, label %5
    i8 34, label %5
  ]

5:                                                ; preds = %1, %1, %5
  %2 = phi i16 [ 0, %1 ], [ 0, %1 ], [ %3, %5 ]
  %3 = add nuw nsw i16 %2, 1
  %6 = icmp eq i16 %3, 7
  br i1 %6, label %9, label %5

9:                                                ; preds = %5, %1
  ret void
}

attributes #0 = { noinline nounwind }"#;

/// The target's phi keeps one incoming per predecessor edge: the two
/// chain blocks the expansion made, plus the backedge.
#[test]
fn two_cases_one_target_keeps_every_incoming() {
    let m = parse_ll(TWO_CASES_LL);
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
    assert_eq!(
        preds.len(),
        3,
        "one incoming per predecessor edge: {preds:?}"
    );
    // The first case's chain block is the switch block itself (it
    // holds the first compare), and the second case's is a fresh
    // chain block, so the phi names both plus its backedge.
    assert!(
        preds.contains(&"1") && preds.contains(&"__switch1"),
        "the phi does not name both chain blocks: {preds:?}"
    );
    for from in &preds {
        let branches_here = handle.blocks.iter().any(|b| {
            (b.label == *from
                && matches!(
                    b.insts.last(),
                    Some(Inst::Br(br)) if br.target == "5"
                ))
                || matches!(
                    b.insts.last(),
                    Some(Inst::BrCond(bc)) if bc.t == "5" || bc.f == "5"
                )
        });
        assert!(branches_here, "the incoming {from} is not a predecessor");
    }
}
