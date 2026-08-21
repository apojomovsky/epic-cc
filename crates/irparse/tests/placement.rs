use irparse::parse_ll;

#[test]
fn reads_the_placement_address_from_a_section_name() {
    let ll = "\
@port = dso_local global i8 0, section \".epicat.0x0F81\", align 1

define dso_local void @main() {
  ret void
}
";
    let m = parse_ll(ll);
    let g = m.globals.iter().find(|g| g.name == "port").unwrap();
    assert_eq!(g.addr, Some(0x0F81));
}

#[test]
fn leaves_addr_none_for_an_unplaced_global() {
    let ll = "\
@x = dso_local global i8 0, align 1

define dso_local void @main() {
  ret void
}
";
    let m = parse_ll(ll);
    let g = m.globals.iter().find(|g| g.name == "x").unwrap();
    assert_eq!(g.addr, None);
}
