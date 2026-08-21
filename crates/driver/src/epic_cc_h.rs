//! The header epic-cc ships to user code. Every macro reduces to
//! `__attribute__((section(...)))`, the one attribute form clang forwards
//! verbatim into the .ll (confirmed against the pinned clang 20.1.8,
//! docs/31 D-2/D-9/§5), so nothing here needs clang's cooperation beyond
//! that one already-probed fact.

pub const EPIC_CC_H: &str = r#"#ifndef EPIC_CC_H
#define EPIC_CC_H

/* Absolute placement: pins a global to a fixed address. epic-cc reads the
 * address back out of the section name; see irparse's EPIC_AT handling. */
#define EPIC_AT(addr) __attribute__((section(".epicat." #addr)))

/* Config words: exactly one EPIC_CONFIG(...) is permitted across the whole
 * program. epic-cc finds it two ways: a cheap raw-text pre-scan (to derive
 * EPIC_FOSC_HZ before clang runs) and, authoritatively, this section-tagged
 * dummy symbol after the whole program is merged. */
#define EPIC_CONFIG(spec) \
    static const char __epic_config[] __attribute__((used, section(".epiccfg." spec))) = spec

/* Derived from the resolved config words; see the driver's pre-scan. Not
 * usable as a link-time-only symbol on purpose: it must work in #if and in
 * a compile-time array bound, so it is a real preprocessor macro. */
#ifndef EPIC_FOSC_HZ
#define EPIC_FOSC_HZ 0
#endif

#define EPIC_NAKED __attribute__((naked))
#define __epic_nop()    asm volatile("nop")
#define __epic_clrwdt() asm volatile("clrwdt")
#define __epic_sleep()  asm volatile("sleep")
#define __epic_di()     asm volatile("bcf INTCON, 7")
#define __epic_ei()     asm volatile("bsf INTCON, 7")

#endif /* EPIC_CC_H */
"#;

#[cfg(test)]
mod tests {
    use super::EPIC_CC_H;

    #[test]
    fn header_has_epic_naked() {
        assert!(EPIC_CC_H.contains("EPIC_NAKED"), "EPIC_CC_H missing EPIC_NAKED");
        assert!(EPIC_CC_H.contains("__epic_nop"), "EPIC_CC_H missing __epic_nop");
        assert!(EPIC_CC_H.contains("__epic_clrwdt"), "EPIC_CC_H missing __epic_clrwdt");
        assert!(EPIC_CC_H.contains("__epic_sleep"), "EPIC_CC_H missing __epic_sleep");
        assert!(EPIC_CC_H.contains("__epic_di"), "EPIC_CC_H missing __epic_di");
        assert!(EPIC_CC_H.contains("__epic_ei"), "EPIC_CC_H missing __epic_ei");
        // All intrinsics must be single opaque asm blocks.
        assert!(EPIC_CC_H.contains("asm volatile(\"nop\")"));
        assert!(EPIC_CC_H.contains("asm volatile(\"clrwdt\")"));
        assert!(EPIC_CC_H.contains("asm volatile(\"sleep\")"));
        assert!(EPIC_CC_H.contains("asm volatile(\"bcf INTCON, 7\")"));
        assert!(EPIC_CC_H.contains("asm volatile(\"bsf INTCON, 7\")"));
        assert!(EPIC_CC_H.contains("__attribute__((naked))"));
    }
}
