//! The XC8-compat toolchain and part macros clang gets on every run.
//! Third-party PIC sources switch on them (m-stack's usb.c errors out
//! without `__XC8`); they ride ahead of the user's `-D`s so a user
//! define keeps precedence by argv position.
//!
//! `__wparam` is empty by design (it must ride as `-D__wparam=`, not a
//! bare `-D__wparam`): XC8 and SDCC spell the PIC18 WREG-parameter
//! attribute `__wparam`, and SDCC's pic16 `stdio.h` declares the user
//! sink `void putchar(char) __wparam` (the corpus `%f` probe defines
//! exactly that so one source compiles under both compilers). Our own
//! formatter treats it as a plain `char` argument.
//!
//! `__interrupt(...)` is SDCC's interrupt-handler keyword widened to the
//! XC8 spellings (docs/46 D-3): empty, `high_priority`/`low_priority`
//! and numeric all reach `__attribute__((interrupt(...)))`, whose value
//! clang preserves in the `.ll` `"interrupt"` function attribute for
//! irparse (0 = compatibility single-vector, 1 = high, 2 = low). Empty
//! means 0 via `__VA_OPT__` (`interrupt(0)`); any spelling present adds
//! `+ 0`, which clang folds before the attribute is read. The priority
//! words ride as plain defines on PIC18 only, expanding to their numbers
//! before the attribute sees them.

use device;

use super::headers::part_macro;

pub fn xc8_predefines(core: device::Core, device_name: &str) -> Vec<String> {
    let mut defs = vec![
        "__XC".to_string(),
        "__XC8".to_string(),
        "__EPIC_CC__".to_string(),
        "__wparam=".to_string(),
        "__interrupt(...)=__attribute__((interrupt(__VA_ARGS__ __VA_OPT__(+) 0)))".to_string(),
    ];
    match core {
        device::Core::Pic14 => defs.push("_PIC14".into()),
        device::Core::Pic14e => {
            defs.push("_PIC14".into());
            defs.push("_PIC14E".into());
        }
        device::Core::Pic18 => {
            defs.push("_PIC18".into());
            defs.push("high_priority=1".into());
            defs.push("low_priority=2".into());
        }
        device::Core::PicBaseline => {
            defs.push("_PIC14".into());
            defs.push("_PIC_BASELINE".into());
        }
    }
    defs.push(part_macro(device_name));
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pic18_part_gets_its_xc8_set() {
        assert_eq!(
            xc8_predefines(device::Core::Pic18, "p18f4550"),
            vec![
                "__XC",
                "__XC8",
                "__EPIC_CC__",
                "__wparam=",
                "__interrupt(...)=__attribute__((interrupt(__VA_ARGS__ __VA_OPT__(+) 0)))",
                "_PIC18",
                "high_priority=1",
                "low_priority=2",
                "_18F4550"
            ]
        );
    }

    #[test]
    fn pic14e_sets_both_pic14_spellings() {
        assert_eq!(
            xc8_predefines(device::Core::Pic14e, "p16f1939"),
            vec![
                "__XC",
                "__XC8",
                "__EPIC_CC__",
                "__wparam=",
                "__interrupt(...)=__attribute__((interrupt(__VA_ARGS__ __VA_OPT__(+) 0)))",
                "_PIC14",
                "_PIC14E",
                "_16F1939"
            ]
        );
    }

    #[test]
    fn plain_pic14_names_the_core_and_part() {
        assert_eq!(
            xc8_predefines(device::Core::Pic14, "p16f877a"),
            vec![
                "__XC",
                "__XC8",
                "__EPIC_CC__",
                "__wparam=",
                "__interrupt(...)=__attribute__((interrupt(__VA_ARGS__ __VA_OPT__(+) 0)))",
                "_PIC14",
                "_16F877A"
            ]
        );
    }
}
