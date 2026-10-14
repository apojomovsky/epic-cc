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
    assert_eq!(g.addr.map(|p| p.byte), Some(0x0F81));
    assert_eq!(g.addr.and_then(|p| p.bit), None);
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

#[test]
fn retains_a_bit_entity_extern_with_its_register_and_bit() {
    let ll = "\
@GIE = external dso_local global i8, section \".epic.bit.0x0B.7\", align 1

define dso_local void @main() {
  store volatile i8 1, ptr @GIE, align 1
  ret void
}
";
    let m = parse_ll(ll);
    let g = m.globals.iter().find(|g| g.name == "GIE").unwrap();
    assert_eq!(g.addr.map(|p| p.byte), Some(0x0B));
    assert_eq!(g.addr.and_then(|p| p.bit), Some(7));
    assert!(!g.is_const);
}

#[test]
fn still_skips_a_section_less_extern() {
    let ll = "\
@ext = external dso_local global i8, align 1

define dso_local void @main() {
  ret void
}
";
    let m = parse_ll(ll);
    assert!(m.globals.iter().all(|g| g.name != "ext"));
}
