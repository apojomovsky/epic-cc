//! epic-cc#479: dense-contiguous switches are preserved as `Switch`
//! terminators only when the caller asks for it (the PIC18 driver does;
//! chain-only backends keep irparse's compare-chain expansion). Sparse
//! or small switches chain either way.

use ir::Inst;
use irparse::{parse_ll, parse_ll_opts};

// clang -O1 output shape for a 7-case i8 dispatch with side effects.
const DENSE_LL: &str = r#"define void @dispatch(i8 noundef zeroext %0) #0 {
  %2 = alloca i8, align 1
  switch i8 %0, label %10 [
    i8 0, label %3
    i8 1, label %4
    i8 2, label %5
    i8 3, label %6
    i8 4, label %7
    i8 5, label %8
    i8 6, label %9
  ]

3:                                                ; preds = %1
  br label %10

4:                                                ; preds = %1
  br label %10

5:                                                ; preds = %1
  br label %10

6:                                                ; preds = %1
  br label %10

7:                                                ; preds = %1
  br label %10

8:                                                ; preds = %1
  br label %10

9:                                                ; preds = %1
  br label %10

10:                                               ; preds = %9, %8, %7, %6, %5, %4, %3, %2, %1
  ret void
}

attributes #0 = { noinline nounwind }"#;

#[test]
fn dense_switch_is_preserved_when_asked() {
    let m = parse_ll_opts(DENSE_LL, true);
    let dispatch = m.funcs.iter().find(|f| f.name == "dispatch").unwrap();
    let sw = dispatch.blocks[0]
        .insts
        .last()
        .expect("switch is the entry block's terminator");
    match sw {
        Inst::Switch(s) => {
            assert_eq!(s.cases.len(), 7);
            assert_eq!(s.cases[0], (0, "3".to_string()));
            assert_eq!(s.default, "10");
        }
        other => panic!("expected a preserved Switch, got {other:?}"),
    }
}

#[test]
fn dense_switch_still_chains_by_default() {
    let m = parse_ll(DENSE_LL);
    let dispatch = m.funcs.iter().find(|f| f.name == "dispatch").unwrap();
    assert!(
        dispatch
            .blocks
            .iter()
            .all(|b| !b.insts.iter().any(|i| matches!(i, Inst::Switch(_)))),
        "no preserved switch without the opt-in"
    );
    // The chain expansion's icmp+brcond shape is present instead.
    assert!(dispatch
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, Inst::BrCond(_)))));
}

#[test]
fn sparse_switch_chains_even_when_asked() {
    // Cases 0,1,9: gapped, so the table would jump wild; irparse must
    // keep the chain expansion regardless of the flag.
    let sparse = DENSE_LL.replace(
        "    i8 2, label %5\n    i8 3, label %6\n    i8 4, label %7\n    i8 5, label %8\n    i8 6, label %9",
        "    i8 9, label %5\n    i8 17, label %6\n    i8 40, label %7\n    i8 80, label %8\n    i8 120, label %9",
    );
    let m = parse_ll_opts(&sparse, true);
    let dispatch = m.funcs.iter().find(|f| f.name == "dispatch").unwrap();
    assert!(
        dispatch
            .blocks
            .iter()
            .all(|b| !b.insts.iter().any(|i| matches!(i, Inst::Switch(_)))),
        "a gapped case list must chain, never table"
    );
}
