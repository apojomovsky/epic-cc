use ir::Inst;
use irparse::parse_ll;

// RAII preservation (#459): clang emits destructor calls on every exit
// path and the pipeline must neither strand nor duplicate them. Switch
// lowering only appends to the current block, so calls on the case and
// default arms stay exactly where the frontend put them.
const SWITCH_CLEANUP: &str = r#"
define void @work(i8 %0) {
  switch i8 %0, label %def [ i8 0, label %z
                             i8 1, label %o ]
z:
  call void @dtor(ptr %g)
  br label %done
o:
  call void @dtor(ptr %h)
  br label %done
def:
  br label %done
done:
  call void @dtor(ptr %g)
  ret void
}
"#;

#[test]
fn switch_splicing_preserves_cleanup_calls() {
    let m = parse_ll(SWITCH_CLEANUP);
    let work = m.funcs.iter().find(|f| f.name == "work").expect("work");
    let mut total = 0;
    for b in &work.blocks {
        for i in &b.insts {
            if matches!(i, Inst::Call(c) if c.func == "dtor") {
                total += 1;
            }
        }
    }
    assert_eq!(total, 3, "one cleanup call per exit path must survive");
    let done = work
        .blocks
        .iter()
        .find(|b| b.label == "done")
        .expect("done");
    assert!(
        done.insts
            .iter()
            .any(|i| matches!(i, Inst::Call(c) if c.func == "dtor")),
        "fallthrough cleanup call stays in its block"
    );
}
