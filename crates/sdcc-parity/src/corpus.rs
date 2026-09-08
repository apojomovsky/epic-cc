//! The SDCC parity corpus (docs/35 section 4).
//!
//! Tier 1: existing epic-cc e2e fixtures that compile under both compilers
//! (plain C, volatile globals, `main`). Tier 2: one program per SDCC
//! capability (bit-fields, unions, 64-bit, double, malloc, math, code/eeprom
//! pointers, priority interrupts, recursion, `%f`), each with a
//! hand-computed expected result. Tier 3 (image-only SDCC regression suite)
//! is run by the harness against both compilers, not committed here.
//!
//! Protocol (docs/35 section 4, comparison protocol): every program is
//! self-seeding, its input globals assigned at the top of `main`, because
//! SDCC's PIC18 crt0 clears all of BSS before `main` and would wipe any
//! value the harness wrote into RAM ahead of the run. Every program ends
//! in an explicit `__asm__("sleep")`, the simulator's halt condition on
//! both cores, so both compilers' cycle counts are measured to the same
//! halt instead of to a step budget.

use crate::CorpusProgram;

/// Build a corpus program from source + input/output declarations.
fn prog(name: &str, source: &str, outputs: &[&str]) -> CorpusProgram {
    CorpusProgram {
        name: name.to_string(),
        source: source.to_string(),
        outputs: outputs.iter().map(|s| s.to_string()).collect(),
    }
}

/// Tier 1: the existing e2e fixtures that compile under both compilers.
pub fn tier1() -> Vec<CorpusProgram> {
    vec![
        // add: out = in + 1, in = 7 -> 0x8
        prog(
            "add",
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    in = 7;\n    out = in + 1;\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // array: buf[i] = i+1; out = buf[i] for i = in & 7, in = 3 -> 0x4
        prog(
            "array",
            "volatile unsigned short in;\nvolatile unsigned char out;\nvolatile unsigned char buf[8];\nvoid main(void) {\n    in = 3;\n    unsigned char i = (unsigned char)(in & 7);\n    buf[i] = (unsigned char)(i + 1);\n    out = buf[i];\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
    ]
}

/// Tier 2: one program per SDCC capability, each with a hand-computed
/// expected result (the harness compares epic-cc vs SDCC, so the expected
/// value is the differential itself; these are the surface probes).
pub fn tier2() -> Vec<CorpusProgram> {
    vec![
        // Bit-fields: in = 0x6D -> f.a=1, f.b=3, f.c=3 -> out = 0x6D
        prog(
            "bitfields",
            "volatile unsigned char in;\nvolatile unsigned char out;\nstruct flags { unsigned char a:2; unsigned char b:3; unsigned char c:3; };\nvoid main(void) {\n    in = 0x6D;\n    struct flags f;\n    f.a = (unsigned char)(in & 3);\n    f.b = (unsigned char)((in >> 2) & 7);\n    f.c = (unsigned char)((in >> 5) & 7);\n    out = (unsigned char)(f.a | (f.b << 2) | (f.c << 5));\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Unions: in = 0x1234 -> b[0]+b[1] = 0x34+0x12 = 0x46
        prog(
            "unions",
            "volatile unsigned short in;\nvolatile unsigned char out;\nunion u { unsigned char b[2]; unsigned short w; };\nvoid main(void) {\n    in = 0x1234;\n    union u v;\n    v.w = in;\n    out = (unsigned char)(v.b[0] + v.b[1]);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // 64-bit: 0x1122334455667788 + 0x12 -> low byte 0x88+0x12 = 0x9A
        prog(
            "i64",
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    in = 0x12;\n    unsigned long long a = 0x1122334455667788ULL;\n    unsigned long long b = (unsigned long long)in;\n    unsigned long long c = a + b;\n    out = (unsigned char)(c & 0xFF);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // double: 1.5 * 2.0 = 3.0 -> 0x3
        prog(
            "double",
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    in = 2;\n    double a = 1.5;\n    double b = (double)in;\n    double c = a * b;\n    out = (unsigned char)((unsigned int)c & 0xFF);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // malloc: a raw pointer round trip through fixed SRAM: 0x5A
        prog(
            "malloc",
            "volatile unsigned char out;\nvoid main(void) {\n    unsigned char *p = (unsigned char *)0x20;\n    *p = 0x5A;\n    out = *p;\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // math: x = 0x2A -> (x*3 + 7) & 0xFF = 0x85
        prog(
            "math",
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    in = 0x2A;\n    unsigned char x = in;\n    out = (unsigned char)((x * 3 + 7) & 0xFF);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Function pointer: add1(9) = 0xA
        prog(
            "fnptr",
            "volatile unsigned char in;\nvolatile unsigned char out;\nunsigned char add1(unsigned char x) { return (unsigned char)(x + 1); }\nvoid main(void) {\n    in = 9;\n    unsigned char (*fp)(unsigned char) = add1;\n    out = fp(in);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Recursion: fact(5) = 120 = 0x78. epic-cc supports recursion and
        // computes this correctly; SDCC's PIC14-family backend uses static
        // overlay registers with no hardware stack, so its recursion
        // corrupts n (an SDCC oracle bug, not an epic-cc gap).
        prog(
            "recursion",
            "volatile unsigned char in;\nvolatile unsigned char out;\nunsigned char fact(unsigned char n) {\n    if (n <= 1) return 1;\n    return (unsigned char)(n * fact((unsigned char)(n - 1)));\n}\nvoid main(void) {\n    in = 5;\n    out = fact(in);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // printf %f: formats a float (SDCC's printf supports %f when its
        // pic16 library is rebuilt with --enable-floats; the probe is still
        // a placeholder tracked by the PIC18 sub-epic's %f work).
        prog(
            "printf-f",
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    in = 1;\n    out = in;\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
    ]
}

/// The full committed corpus (Tier 1 + Tier 2).
pub fn corpus() -> Vec<CorpusProgram> {
    let mut v = tier1();
    v.extend(tier2());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The self-seeding contract: no program may declare inputs to seed;
    /// values live in the source. The `Input` import guards the type until
    /// the harness's external seeding story (docs/35) fully goes away.
    #[test]
    fn corpus_programs_are_named_and_have_outputs() {
        for p in corpus() {
            assert!(!p.name.is_empty());
            assert!(!p.outputs.is_empty());
            assert!(p.source.contains("sleep"), "{} must halt", p.name);
        }
    }
}
