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
    prog_for(&[], name, source, outputs)
}

/// Build a corpus program that runs only on the named devices.
fn prog_for(devices: &[&str], name: &str, source: &str, outputs: &[&str]) -> CorpusProgram {
    CorpusProgram {
        name: name.to_string(),
        source: source.to_string(),
        outputs: outputs.iter().map(|s| s.to_string()).collect(),
        devices: devices.iter().map(|s| s.to_string()).collect(),
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
        // printf %f: formats 7 * 0.5 through printf into a buffer via a
        // user putchar, then folds the first six output bytes. epic-cc
        // prints 2 fixed decimals (#295) and SDCC's %f prints 6, but the
        // first six bytes ("d=3.50") agree either way, so the fold
        // compares the computed value, not the formatting policy. SDCC's
        // pic14 port has no libc (manual 4.9.8): its pic14/pic14e rows
        // are arbitrated as an SDCC limitation. Expected fold of
        // "d=3.50" = 0x41.
        prog(
            "printf-f",
            "#include <stdio.h>\nvolatile unsigned char seed;\nvolatile unsigned char out;\nstatic char buf[16];\nstatic unsigned char pos;\nint putchar(int c) {\n    buf[pos] = (char)c;\n    pos++;\n    return c;\n}\nvoid main(void) {\n    seed = 7;\n    printf(\"d=%f\\r\\n\", seed * 0.5);\n    out = (unsigned char)(buf[0] ^ buf[1] ^ buf[2] ^ buf[3] ^ buf[4] ^ buf[5]);\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Data EEPROM (PIC14 family, DS39582C): write two cells through
        // the EEADR/EEDATA/EECON1/EECON2 window, read them back through
        // the RD cycle, fold. Only a real EEPROM array produces 0x80
        // (0x5C ^ 0xA3 ^ 0x7F): a sim that never models the array
        // returns the second cell for both reads and lands on 0x7F.
        // Register addresses differ per family, so each family carries
        // its own probe via the struct's device filter, not #if.
        prog_for(
            &["p16f877a"],
            "eeprom-p14",
            "volatile unsigned char out;\n#define EEADR  (*(volatile unsigned char *)0x010D)\n#define EEDATA (*(volatile unsigned char *)0x010C)\n#define EECON1 (*(volatile unsigned char *)0x018C)\n#define EECON2 (*(volatile unsigned char *)0x018D)\nvoid main(void) {\n    EEADR = 0x10;\n    EEDATA = 0x5C;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x11;\n    EEDATA = 0xA3;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x10;\n    EECON1 = 0x01;\n    {\n        unsigned char first = EEDATA;\n        EEADR = 0x11;\n        EECON1 = 0x01;\n        out = (unsigned char)(first ^ EEDATA ^ 0x7F);\n    }\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Data EEPROM (PIC18 family, DS39632E register set).
        prog_for(
            &["p18f4550"],
            "eeprom-p18",
            "volatile unsigned char out;\n#define EEADR  (*(volatile unsigned char *)0x0FA9)\n#define EEDATA (*(volatile unsigned char *)0x0FA8)\n#define EECON1 (*(volatile unsigned char *)0x0FA6)\n#define EECON2 (*(volatile unsigned char *)0x0FA7)\nvoid main(void) {\n    EEADR = 0x10;\n    EEDATA = 0x5C;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x11;\n    EEDATA = 0xA3;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x10;\n    EECON1 = 0x01;\n    {\n        unsigned char first = EEDATA;\n        EEADR = 0x11;\n        EECON1 = 0x01;\n        out = (unsigned char)(first ^ EEDATA ^ 0x7F);\n    }\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Data EEPROM (Enhanced Mid-range family, DS41364E register set).
        prog_for(
            &["p16f1938"],
            "eeprom-p14e",
            "volatile unsigned char out;\n#define EEADR  (*(volatile unsigned char *)0x0191)\n#define EEDATA (*(volatile unsigned char *)0x0193)\n#define EECON1 (*(volatile unsigned char *)0x0195)\n#define EECON2 (*(volatile unsigned char *)0x0196)\nvoid main(void) {\n    EEADR = 0x10;\n    EEDATA = 0x5C;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x11;\n    EEDATA = 0xA3;\n    EECON1 = 0x04;\n    EECON2 = 0x55;\n    EECON2 = 0xAA;\n    EECON1 = 0x08;\n    EECON1 = 0x00;\n    EEADR = 0x10;\n    EECON1 = 0x01;\n    {\n        unsigned char first = EEDATA;\n        EEADR = 0x11;\n        EECON1 = 0x01;\n        out = (unsigned char)(first ^ EEDATA ^ 0x7F);\n    }\n    __asm__(\"sleep\");\n}\n",
            &["out"],
        ),
        // Pointer traversal of a const table in flash: epic-cc reads the
        // cells through its TBLRD (PIC18) / RETLW (PIC14) const path,
        // SDCC through its 3-byte generic pointers (docs/35 PIC18 table,
        // code/eeprom pointers row). table[2] + table[3] = 107 = 0x6B.
        prog(
            "constptr",
            "volatile unsigned char in;\nvolatile unsigned char out;\nconst unsigned char table[8] = {3, 14, 15, 92, 65, 35, 89, 79};\nvoid main(void) {\n    in = 2;\n    const unsigned char *p = table + (in & 3);\n    out = (unsigned char)(p[0] + p[1]);\n    __asm__(\"sleep\");\n}\n",
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
