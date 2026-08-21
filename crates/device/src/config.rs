//! `EPIC_CONFIG("...")` string parsing and resolution against a device's
//! `ConfigRegion`. Pure data in, `Vec<u8>` out: no IR, no driver dependency.

use crate::ConfigRegion;

/// Resolve a comma-separated `key=value, key=value` spec against `region`,
/// starting from `region.erased_baseline` and applying each mentioned
/// field, each unmentioned field's default, in that order.
///
/// Panics if: a required field (`default: None`) is never mentioned; a
/// mentioned field name does not exist in `region`; a mentioned value name
/// does not exist for that field; a field is `locked` to a different value
/// than the one given.
pub fn resolve_config(region: &ConfigRegion, spec: &str) -> Vec<u8> {
    let mut bytes = region.erased_baseline.to_vec();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, val) = pair
            .split_once('=')
            .unwrap_or_else(|| panic!("device: malformed EPIC_CONFIG entry {pair:?} (expected key=value)"));
        let (key, val) = (key.trim(), val.trim());

        let field = region
            .fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(key))
            .unwrap_or_else(|| panic!("device: unknown field '{key}' in EPIC_CONFIG"));

        if let Some(only) = field.locked {
            if !val.eq_ignore_ascii_case(only) {
                panic!(
                    "device: field '{}' is locked to {only:?} (epic-cc's backend cannot honor \
                     other values); got {val:?}",
                    field.name
                );
            }
        }

        let fv = field
            .values
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(val))
            .unwrap_or_else(|| {
                let opts: Vec<&str> = field.values.iter().map(|v| v.name).collect();
                panic!(
                    "device: unknown value '{val}' for field '{}', expected one of {opts:?}",
                    field.name
                )
            });

        apply(&mut bytes, field, fv.bits);
        seen.insert(field.name);
    }

    for field in region.fields {
        if seen.contains(field.name) {
            continue;
        }
        let default_name = field.default.unwrap_or_else(|| {
            panic!(
                "device: field '{}' has no default and was not set by EPIC_CONFIG; \
                 this device cannot boot without an explicit value. Valid values: {:?}",
                field.name,
                field.values.iter().map(|v| v.name).collect::<Vec<_>>()
            )
        });
        let fv = field
            .values
            .iter()
            .find(|v| v.name == default_name)
            .unwrap_or_else(|| panic!("device: field {:?}'s own default {default_name:?} is not one of its values (data bug)", field.name));
        apply(&mut bytes, field, fv.bits);
    }

    bytes
}

fn apply(bytes: &mut [u8], field: &crate::FuseField, bits: u8) {
    let i = field.byte_offset as usize;
    bytes[i] = (bytes[i] & !field.mask) | ((bits << field.shift) & field.mask);
}
