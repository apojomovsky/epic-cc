//! epic-cc#1417: a switch whose default label is also a case target
//! reaches that target twice, so its phi needs an incoming for each
//! edge. The default's edge comes from the last chain block's false
//! arm, the case's from its true arm.

use ir::Inst;
use irparse::parse_ll;

const DEFAULT_IS_CASE_LL: &str = r#"define void @handle(i8 noundef zeroext %0) #0 {
  switch i8 %0, label %5 [
    i8 32, label %5
  ]

5:                                                ; preds = %1, %1, %5
  %6 = phi i16 [ 0, %1 ], [ 0, %1 ], [ %7, %5 ]
  %7 = add nuw nsw i16 %6, 1
  %8 = icmp eq i16 %7, 7
  br i1 %8, label %9, label %5

9:                                                ; preds = %5, %1
  ret void
}

attributes #0 = { noinline nounwind }"#;

/// The target's phi names both edges the switch reaches it with, plus
/// its backedge.
#[test]
fn default_also_a_case_target_keeps_every_incoming() {
    let m = parse_ll(DEFAULT_IS_CASE_LL);
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
