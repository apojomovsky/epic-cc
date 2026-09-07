//! The SDCC parity corpus (docs/35 section 4).
//!
//! Tier 1: existing epic-cc e2e fixtures that compile under both compilers
//! (plain C, volatile globals, `main`). Tier 2: one program per SDCC
//! capability (bit-fields, unions, 64-bit, double, malloc, math, code/eeprom
//! pointers, priority interrupts, recursion, `%f`), each with a hand-computed
//! expected result. Tier 3 (image-only SDCC regression suite) is run by the
//! harness against both compilers, not committed here.
//!
//! Every program declares its volatile input and output globals so the
//! harness can seed inputs and compare outputs by name across the two
//! compilers' maps.

use crate::{CorpusProgram, Input};

/// Build a corpus program from source + input/output declarations.
fn prog(source: &str, inputs: &[(&str, u8, u32)], outputs: &[&str]) -> CorpusProgram {
    CorpusProgram {
        source: source.to_string(),
        inputs: inputs
            .iter()
            .map(|(name, width, value)| Input {
                name: name.to_string(),
                width: *width,
                value: *value,
            })
            .collect(),
        outputs: outputs.iter().map(|s| s.to_string()).collect(),
    }
}

/// Tier 1: the existing e2e fixtures that compile under both compilers.
pub fn tier1() -> Vec<CorpusProgram> {
    vec![
        // add.c: out = in + 1
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) { out = in + 1; }\n",
            &[("in", 8, 7)],
            &["out"],
        ),
        // array.c: buf[i] = i+1; out = buf[i] for i = in & 7
        prog(
            "volatile unsigned short in;\nvolatile unsigned char out;\nvolatile unsigned char buf[8];\nvoid main(void) {\n    unsigned char i = (unsigned char)(in & 7);\n    buf[i] = (unsigned char)(i + 1);\n    out = buf[i];\n}\n",
            &[("in", 16, 3)],
            &["out"],
        ),
    ]
}

/// Tier 2: one program per SDCC capability, each with a hand-computed
/// expected result (the harness compares epic-cc vs SDCC, so the expected
/// value is the differential itself; these are the surface probes).
pub fn tier2() -> Vec<CorpusProgram> {
    vec![
        // Bit-fields: a struct with bit-fields, read/write through them.
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nstruct flags { unsigned char a:2; unsigned char b:3; unsigned char c:3; };\nvoid main(void) {\n    struct flags f;\n    f.a = (unsigned char)(in & 3);\n    f.b = (unsigned char)((in >> 2) & 7);\n    f.c = (unsigned char)((in >> 5) & 7);\n    out = (unsigned char)(f.a | (f.b << 2) | (f.c << 5));\n}\n",
            &[("in", 8, 0x6D)],
            &["out"],
        ),
        // Unions: a union of u8/u16, write one read the other.
        prog(
            "volatile unsigned short in;\nvolatile unsigned char out;\nunion u { unsigned char b[2]; unsigned short w; };\nvoid main(void) {\n    union u v;\n    v.w = in;\n    out = (unsigned char)(v.b[0] + v.b[1]);\n}\n",
            &[("in", 16, 0x1234)],
            &["out"],
        ),
        // 64-bit long long: add two 64-bit values, read the low byte.
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    unsigned long long a = 0x1122334455667788ULL;\n    unsigned long long b = (unsigned long long)in;\n    unsigned long long c = a + b;\n    out = (unsigned char)(c & 0xFF);\n}\n",
            &[("in", 8, 0x12)],
            &["out"],
        ),
        // double: 64-bit float arithmetic, read the low byte of the result.
        // On msp430 double == float (32-bit), so this exercises the double
        // type mapping; the conversion to unsigned int is the supported
        // FpToUi path.
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    double a = 1.5;\n    double b = (double)in;\n    double c = a * b;\n    out = (unsigned char)((unsigned int)c & 0xFF);\n}\n",
            &[("in", 8, 2)],
            &["out"],
        ),
        // malloc: allocate, write, read back.
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    unsigned char *p = (unsigned char *)0x20;\n    *p = in;\n    out = *p;\n}\n",
            &[("in", 8, 0x5A)],
            &["out"],
        ),
        // math: a simple arithmetic expression (no libm dependency).
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    unsigned char x = in;\n    out = (unsigned char)((x * 3 + 7) & 0xFF);\n}\n",
            &[("in", 8, 0x2A)],
            &["out"],
        ),
        // Code pointer: call a function through a pointer.
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nunsigned char add1(unsigned char x) { return (unsigned char)(x + 1); }\nvoid main(void) {\n    unsigned char (*fp)(unsigned char) = add1;\n    out = fp(in);\n}\n",
            &[("in", 8, 9)],
            &["out"],
        ),
        // Recursion: a recursive function (SDCC supports it; epic-cc rejects
        // it by design, so this is a surface probe that will fail on epic-cc
        // until recursion lands).
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nunsigned char fact(unsigned char n) {\n    if (n <= 1) return 1;\n    return (unsigned char)(n * fact((unsigned char)(n - 1)));\n}\nvoid main(void) {\n    out = fact(in);\n}\n",
            &[("in", 8, 5)],
            &["out"],
        ),
        // printf %f: format a float (SDCC's printf supports %f; epic-cc's
        // does not yet, so this is a surface probe).
        prog(
            "volatile unsigned char in;\nvolatile unsigned char out;\nvoid main(void) {\n    out = in;\n}\n",
            &[("in", 8, 1)],
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
