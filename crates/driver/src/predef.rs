//! The XC8-compat toolchain and part macros clang gets on every run.
//! Third-party PIC sources switch on them (m-stack's usb.c errors out
//! without `__XC8`); they ride ahead of the user's `-D`s so a user
//! define keeps precedence by argv position.

use device;

pub fn xc8_predefines(core: device::Core, device_name: &str) -> Vec<String> {
    let mut defs = vec!["__XC".to_string(), "__XC8".to_string()];
    match core {
        device::Core::Pic14 => defs.push("_PIC14".into()),
        device::Core::Pic14e => {
            defs.push("_PIC14".into());
            defs.push("_PIC14E".into());
        }
        device::Core::Pic18 => defs.push("_PIC18".into()),
    }
    let part = device_name
        .strip_prefix('p')
        .unwrap_or(device_name)
        .to_uppercase();
    defs.push(format!("_{part}"));
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pic18_part_gets_its_xc8_set() {
        assert_eq!(
            xc8_predefines(device::Core::Pic18, "p18f4550"),
            vec!["__XC", "__XC8", "_PIC18", "_18F4550"]
        );
    }

    #[test]
    fn pic14e_sets_both_pic14_spellings() {
        assert_eq!(
            xc8_predefines(device::Core::Pic14e, "p16f1939"),
            vec!["__XC", "__XC8", "_PIC14", "_PIC14E", "_16F1939"]
        );
    }

    #[test]
    fn plain_pic14_names_the_core_and_part() {
        assert_eq!(
            xc8_predefines(device::Core::Pic14, "p16f877a"),
            vec!["__XC", "__XC8", "_PIC14", "_16F877A"]
        );
    }
}
