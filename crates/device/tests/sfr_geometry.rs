use std::collections::HashMap;

// SFR bitfield geometry the header generator trusts: widths the emitter
// knows (1, 2, 3), contiguous masks, no same-mode overlap, nothing past
// bit 7. A regen that breaks one would silently misposition a member.
#[test]
fn sfr_field_geometry_holds_on_every_device() {
    for dev in device::ALL {
        for s in dev.sfrs {
            assert!(
                [1, 2, 3].contains(&s.width),
                "{}: sfr {:?} width {}",
                dev.name,
                s.name,
                s.width
            );
            let mut modes: HashMap<u8, u8> = HashMap::new();
            for f in s.fields {
                assert!(
                    f.mask != 0,
                    "{}: {:?}.{:?} mask 0",
                    dev.name,
                    s.name,
                    f.name
                );
                assert!(f.shift < 8, "{}: {:?}.{:?} shift", dev.name, s.name, f.name);
                let width = f.mask.count_ones();
                assert_eq!(
                    f.mask as u16 >> f.shift,
                    (1 << width) - 1,
                    "{}: {:?}.{:?} mask {:#04X} not contiguous",
                    dev.name,
                    s.name,
                    f.name,
                    f.mask
                );
                assert!(
                    f.shift as u32 + width <= 8,
                    "{}: {:?}.{:?} past bit 7",
                    dev.name,
                    s.name,
                    f.name
                );
                let slot = modes.entry(f.mode).or_insert(0);
                assert!(
                    *slot & f.mask == 0,
                    "{}: {:?}.{:?} overlaps mode-{}",
                    dev.name,
                    s.name,
                    f.name,
                    f.mode
                );
                *slot |= f.mask;
            }
        }
    }
}
