//! `EPIC_CONFIG("...")` string parsing and resolution against a device's
//! `ConfigRegion`. Pure data in, `Vec<u8>` out: no IR, no driver dependency.

use crate::ConfigRegion;

/// Which spelling the spec came from. `#pragma config` leaves an omitted
/// field without a default at its erased value, the way XC8 does;
/// `EPIC_CONFIG` keeps the strict rule and errors instead (epic-cc#706).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigSpelling {
    EpicConfig,
    Pragma,
}

impl ConfigSpelling {
    fn name(&self) -> &'static str {
        match self {
            ConfigSpelling::EpicConfig => "EPIC_CONFIG",
            ConfigSpelling::Pragma => "#pragma config",
        }
    }
}

/// Resolve a comma-separated `key=value, key=value` spec against `region`,
/// starting from `region.erased_baseline` and applying each mentioned
/// field, each unmentioned field's default, in that order. A field or value
/// may be named by any of its pack aliases (`FOSC=HS`), ignoring case.
///
/// Panics if: a required field (`default: None`) is never mentioned; a
/// mentioned field name does not exist in `region`; a mentioned value name
/// does not exist for that field; a field is `locked` to a different value
/// than the one given.
pub fn resolve_config(region: &ConfigRegion, spec: &str) -> Vec<u8> {
    try_resolve_config(region, spec).unwrap_or_else(|e| panic!("epic-cc: error: {e}"))
}

/// Fallible `resolve_config`, so the driver can point the error at the
/// `EPIC_CONFIG` site instead of panicking bare.
pub fn try_resolve_config(region: &ConfigRegion, spec: &str) -> Result<Vec<u8>, String> {
    try_resolve_config_in(region, spec, ConfigSpelling::EpicConfig)
}

/// Fallible `resolve_config` for a known spelling: the errors name it,
/// and `#pragma config` lets an omitted field without a default keep the
/// erased baseline the bytes start from.
pub fn try_resolve_config_in(
    region: &ConfigRegion,
    spec: &str,
    spelling: ConfigSpelling,
) -> Result<Vec<u8>, String> {
    let sp = spelling.name();
    let mut bytes = region.erased_baseline.to_vec();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, val) = pair
            .split_once('=')
            .ok_or_else(|| format!("malformed {sp} entry {pair:?} (expected key=value)"))?;
        let (key, val) = (key.trim(), val.trim());

        let field = region
            .fields
            .iter()
            .find(|f| names_match(f.name, f.aliases, key))
            .ok_or_else(|| {
                let names: Vec<&str> = region
                    .fields
                    .iter()
                    .flat_map(|f| std::iter::once(f.name).chain(f.aliases.iter().copied()))
                    .collect();
                format!(
                    "unknown field '{key}' in {sp} (expected one of: {})",
                    names.join(", ")
                )
            })?;

        let fv = field
            .values
            .iter()
            .find(|v| names_match(v.name, v.aliases, val))
            .ok_or_else(|| {
                let opts: Vec<&str> = field
                    .values
                    .iter()
                    .flat_map(|v| std::iter::once(v.name).chain(v.aliases.iter().copied()))
                    .collect();
                format!(
                    "unknown value '{val}' for field '{}' in {sp}, expected one of {opts:?}",
                    field.name
                )
            })?;

        if let Some(only) = field.locked {
            if !fv.name.eq_ignore_ascii_case(only) {
                return Err(format!(
                    "field '{}' is locked to {only:?} (epic-cc's backend cannot honor \
                     other values); got {val:?} via {sp}",
                    field.name
                ));
            }
        }

        apply(&mut bytes, field, fv.bits);
        seen.insert(field.name);
    }

    for field in region.fields {
        if seen.contains(field.name) {
            continue;
        }
        let Some(default_name) = field.default else {
            if spelling == ConfigSpelling::Pragma {
                continue;
            }
            return Err(format!(
                "field '{}' has no default and was not set by EPIC_CONFIG; \
                 this device cannot boot without an explicit value. Valid values: {:?}",
                field.name,
                field.values.iter().map(|v| v.name).collect::<Vec<_>>()
            ));
        };
        let fv = field
            .values
            .iter()
            .find(|v| v.name == default_name)
            .ok_or_else(|| {
                format!(
                    "field {:?}'s own default {default_name:?} is not one of its values (data bug)",
                    field.name
                )
            })?;
        apply(&mut bytes, field, fv.bits);
    }

    Ok(bytes)
}

/// Find the config field named `name`, matching normalized names and pack
/// aliases case-insensitively (`FOSC` finds `osc`). Used where a spec may
/// use either spelling: the driver's clock derivation over `#pragma
/// config` pairs.
pub fn find_field<'a>(region: &'a ConfigRegion, name: &str) -> Option<&'a crate::FuseField> {
    region
        .fields
        .iter()
        .find(|f| names_match(f.name, f.aliases, name))
}

fn names_match(name: &str, aliases: &[&str], given: &str) -> bool {
    name.eq_ignore_ascii_case(given) || aliases.iter().any(|a| a.eq_ignore_ascii_case(given))
}

fn apply(bytes: &mut [u8], field: &crate::FuseField, bits: u8) {
    let i = field.byte_offset as usize;
    bytes[i] = (bytes[i] & !field.mask) | ((bits << field.shift) & field.mask);
}
