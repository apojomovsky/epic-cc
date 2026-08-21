use device::{resolve_config, PIC16F877A, PIC18F4550};

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

#[test]
fn pic18_erased_baseline_is_all_ff_confirmed_against_gpasm() {
    // Confirmed empirically 2026-08-21: assembling CONFIG4L through gpasm
    // 1.5.2 with every named field set left both genuinely-unimplemented
    // bits AND the untouched gap byte 0x300007 at 0xFF, not the "reads as
    // 0" value DS39632E's register legends state (that describes SFR
    // read-time masking, not what gets written to flash).
    assert_eq!(PIC18F4550.config.erased_baseline, &[0xFF; 14]);
    assert_eq!(PIC18F4550.config.num_bytes, 14);
    assert_eq!(PIC18F4550.config.base_byte_addr, 0x300000);
}

#[test]
fn xinst_is_locked_off() {
    let f = PIC18F4550
        .config
        .fields
        .iter()
        .find(|f| f.name == "xinst")
        .unwrap();
    assert_eq!(f.locked, Some("off"));
}

#[test]
#[should_panic(expected = "field 'xinst' is locked to \"off\"")]
fn overriding_xinst_on_panics() {
    resolve_config(
        &PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, xinst=on",
    );
}

#[test]
fn resolves_config4l_and_matches_gpasm() {
    // gpasm 1.5.2, 2026-08-21: __CONFIG _CONFIG4L, _DEBUG_OFF_4L &
    // _XINST_OFF_4L & _ICPRT_OFF_4L & _LVP_OFF_4L & _STVREN_ON_4L -> 0x9B
    // at byte offset 6 (CONFIG4L, address 0x300006).
    let bytes = resolve_config(
        &PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, \
         debug=off, xinst=off, icprt=off, lvp=off, stvren=on",
    );
    assert_eq!(bytes[6], 0x9B);
    // The gap byte right after it stays at the erased baseline: gpasm's
    // own output for this test showed 0x300007 = 0xFF, untouched.
    assert_eq!(bytes[7], 0xFF);
}
