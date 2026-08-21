use device::{resolve_config, PIC16F877A};

#[test]
fn erased_baseline_is_the_datasheet_stated_value() {
    // DS39582C Register 14-1, note 1: "the erased (unprogrammed) value of
    // the Configuration Word is 3FFFh" (low byte 0xFF, high byte 0x3F).
    assert_eq!(PIC16F877A.config.erased_baseline, &[0xFF, 0x3F]);
}

#[test]
fn resolves_a_representative_override_and_matches_hand_computation() {
    // osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off,
    // debug=off, cp=off. Hand-computed against DS39582C Register 14-1 and
    // cross-checked against gpasm 1.5.2 (2026-08-21): word 0x3F71.
    let bytes = resolve_config(
        &PIC16F877A.config,
        "osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off, debug=off, cp=off",
    );
    assert_eq!(bytes, vec![0x71, 0x3F]);
}

#[test]
#[should_panic(expected = "field 'osc' has no default")]
fn panics_when_the_required_oscillator_field_is_missing() {
    resolve_config(&PIC16F877A.config, "wdt=off");
}

#[test]
#[should_panic(expected = "unknown field 'wat'")]
fn panics_on_an_unknown_field() {
    resolve_config(&PIC16F877A.config, "osc=xt, wat=off");
}

#[test]
#[should_panic(expected = "unknown value 'turbo' for field 'osc'")]
fn panics_on_an_unknown_value() {
    resolve_config(&PIC16F877A.config, "osc=turbo");
}

#[test]
fn unmentioned_fields_take_their_default() {
    // Only osc set (required); everything else should resolve to its
    // stated default, matching the full-override test's non-osc bytes
    // exactly, since PIC16F877A.config's defaults ARE that combination.
    let bytes = resolve_config(&PIC16F877A.config, "osc=xt");
    assert_eq!(bytes, vec![0x71, 0x3F]);
}
