use iselcore::{ssa_key, Slot};

#[test]
fn slot_direct_returns_the_address() {
    assert_eq!(Slot::Direct(0x42).direct(), 0x42);
}

#[test]
fn ssa_key_joins_function_and_value_names() {
    assert_eq!(ssa_key("main", "1"), "main::1");
}
