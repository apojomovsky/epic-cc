//! epic-cc#1417: a switch that irparse preserves as a dense run with a
//! residual chain behind it. The preserved run's edges still leave the
//! switch block as the case target's predecessor, so only the residual
//! edge's phi incoming moves.

use ir::Inst;
use irparse::parse_ll_opts;

/// A switch whose cases 0..5 form a dense run and whose case 100 is
/// the leftover the residual chain handles. The case target carries a
/// phi with one entry per edge: seven from the switch block and one
/// from the backedge.
const RESIDUAL_LL: &str = r#"define void @handle(i8 noundef zeroext %0) #0 {
  switch i8 %0, label %9 [
    i8 0, label %5
    i8 1, label %5
    i8 2, label %5
    i8 3, label %5
    i8 4, label %5
    i8 5, label %5
    i8 100, label %5
  ]

5:                                                ; preds = %1, %1, %1, %1, %1, %1, %1, %5
  %6 = phi i16 [ 0, %1 ], [ 0, %1 ], [ 0, %1 ], [ 0, %1 ], [ 0, %1 ], [ 0, %1 ], [ 0, %1 ], [ %7, %5 ]
  %7 = add nuw nsw i16 %6, 1
  %8 = icmp eq i16 %7, 7
  br i1 %8, label %9, label %5

9:                                                ; preds = %5, %1
  ret void
}

attributes #0 = { noinline nounwind }"#;

/// The residual edge's incoming names the residual chain block, and the
/// preserved run's edges keep naming the switch block, so the phi
/// still holds one entry per edge.
#[test]
fn residual_edge_moves_and_the_preserved_run_stays() {
    let m = parse_ll_opts(RESIDUAL_LL, true);
    let handle = m.funcs.iter().find(|f| f.name == "handle").unwrap();
    // The dense run is preserved, so the switch block holds a `Switch`
    // terminator and the residual chain follows it.
    let residual = handle
        .blocks
        .iter()
        .find(|b| b.label.starts_with("__switch"))
        .expect("the residual chain block");
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
        8,
        "one incoming per predecessor edge: {preds:?}"
    );
    assert!(
        preds.iter().any(|p| *p == residual.label),
        "the residual edge's incoming did not move: {preds:?}"
    );
    // The preserved run's edges still leave the switch block, so their
    // incomings keep naming it. The run holds cases 0..5, so six
    // edges.
    assert_eq!(
        preds.iter().filter(|p| **p == "1").count(),
        6,
        "the preserved run's incomings moved: {preds:?}"
    );
}
