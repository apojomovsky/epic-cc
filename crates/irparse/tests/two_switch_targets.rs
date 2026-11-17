//! Two switches in one function whose case target is the same block:
//! the target's phi names both switch blocks, and each expansion
//! re-points one of them to its own chain blocks.

use ir::Inst;
use irparse::parse_ll;

const TWO_SWITCH_LL: &str = r#"define void @handle(i8 noundef zeroext %0, i8 noundef zeroext %1) #0 {
  switch i8 %0, label %7 [
    i8 32, label %4
    i8 34, label %5
  ]

3:                                                ; preds = %2
  switch i8 %1, label %7 [
    i8 33, label %6
    i8 35, label %5
  ]

4:                                                ; preds = %2
  br label %7

5:                                                ; preds = %2, %3, %5
  %idx = phi i16 [ 0, %2 ], [ 0, %3 ], [ %next, %5 ]
  %next = add nuw nsw i16 %idx, 1
  %8 = icmp eq i16 %next, 7
  br i1 %8, label %7, label %5

6:                                                ; preds = %3
  br label %7

7:                                                ; preds = %6, %5, %4, %3, %2
  ret void
}

attributes #0 = { noinline nounwind }"#;

/// Every incoming of the shared case target names a block that really
/// branches to it, so the backend finds each edge's copy.
#[test]
fn two_switches_one_target_keeps_every_incoming() {
    let m = parse_ll(TWO_SWITCH_LL);
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
    // Each switch re-pointed its own case, so the target's phi names
    // one chain block per switch plus its backedge.
    let chain = preds.iter().filter(|p| p.starts_with("__switch")).count();
    assert_eq!(chain, 2, "one chain block per switch: {preds:?}");
    assert!(
        preds.contains(&"5"),
        "the phi lost its backedge incoming: {preds:?}"
    );
}
