//! Rule-by-rule tests for the PIC18 code-factoring pass on micro-listings.

use outline::{factor, factor_with_locs, Options};

/// A function body: `label`, the given lines, then `RETURN`.
fn func(name: &str, body: &[&str]) -> String {
    let mut s = format!("{name}:\n");
    for l in body {
        s.push_str(&format!("    {l}\n"));
    }
    s.push_str("    RETURN\n");
    s
}

/// `__start` calls every named function, then the functions follow.
fn program(funcs: &[String], callees: &[&str]) -> String {
    let mut s = String::from("    org 0x0000\n    goto __start\n");
    for f in funcs {
        s.push_str(f);
    }
    s.push_str("__start:\n");
    for c in callees {
        s.push_str(&format!("    CALL {c}\n"));
    }
    s.push_str("    SLEEP\n    end\n");
    s
}

const RUN: [&str; 4] = [
    "MOVF 0x020,W,A",
    "ADDWF 0x021,W,A",
    "MOVWF 0x022,A",
    "CLRF 0x023,A",
];

fn with_run(name: &str, extra: &str) -> String {
    let mut body: Vec<&str> = vec![extra];
    body.extend(RUN);
    body.push(extra);
    func(name, &body)
}

fn calls_to(out: &str, body: &str) -> usize {
    out.lines()
        .filter(|l| {
            let t = l.trim();
            (t.starts_with("RCALL ") || t.starts_with("CALL ")) && t.ends_with(body)
        })
        .count()
}

#[test]
fn repeated_run_becomes_one_shared_body() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let out = factor(&src, &Options::default());
    assert!(out.contains("__pa0:"), "{out}");
    assert_eq!(calls_to(&out, "__pa0"), 3, "{out}");
    assert_eq!(
        out.matches("ADDWF 0x021,W,A").count(),
        1,
        "one copy left: {out}"
    );
}

#[test]
fn cross_function_bodies_use_long_calls_and_go_before_end() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let out = factor(&src, &Options::default());
    assert!(out.contains("    CALL __pa0"), "{out}");
    let body = out.find("__pa0:").unwrap();
    let end = out.rfind("    end").unwrap();
    let sleep = out.find("SLEEP").unwrap();
    assert!(
        sleep < body && body < end,
        "body sits after the code, before end: {out}"
    );
}

#[test]
fn local_repeats_use_rcall_and_sit_after_their_function() {
    let mut body: Vec<&str> = Vec::new();
    for sep in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"] {
        body.push(sep);
        body.extend(RUN);
    }
    let src = program(&[func("f", &body), func("g", &["NOP"])], &["f", "g"]);
    let out = factor(&src, &Options::default());
    assert_eq!(calls_to(&out, "__pa0"), 3, "{out}");
    assert!(out.contains("    RCALL __pa0"), "{out}");
    let ret = out.find("    RETURN\n").unwrap();
    let body_at = out.find("__pa0:").unwrap();
    let g = out.find("g:").unwrap();
    assert!(
        ret < body_at && body_at < g,
        "body between f's RETURN and g: {out}"
    );
}

#[test]
fn skip_successor_is_never_a_site_start_even_across_a_label() {
    let mut src = String::new();
    for (n, sep) in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"]
        .iter()
        .enumerate()
    {
        let mut b = vec![*sep, "BTFSC 0x040,0,A"];
        let lab = format!("L{n}:");
        let _ = lab;
        b.extend(RUN);
        src.push_str(&func(&format!("f{n}"), &b));
    }
    // Put a label between each skip and its successor.
    let src = src.replace("BTFSC 0x040,0,A\n", "BTFSC 0x040,0,A\nmid:\n");
    let prog = program(&[src], &["f0", "f1", "f2"]);
    let out = factor(&prog, &Options::default());
    for l in out.lines().collect::<Vec<_>>().windows(3) {
        if l[0].trim() == "BTFSC 0x040,0,A" {
            let succ = if l[1].ends_with(':') { l[2] } else { l[1] };
            assert!(!succ.contains("__pa"), "skip successor replaced: {out}");
        }
    }
}

#[test]
fn stack_and_pc_registers_are_never_moved() {
    for reg in ["0xFF9,A", "0xF9,A", "0xFFD", "0xFD,A", "TOSL", "0xFA,B"] {
        let run = [
            "MOVF 0x020,W,A".to_string(),
            format!("MOVWF {reg}"),
            "CLRF 0x023,A".to_string(),
            "INCF 0x024,F,A".to_string(),
        ];
        let run: Vec<&str> = run.iter().map(String::as_str).collect();
        let mk = |n: &str, sep: &str| {
            let mut b = vec![sep];
            b.extend(run.iter().copied());
            b.push(sep);
            func(n, &b)
        };
        let src = format!(
            "TOSL equ 0xFFD\n{}",
            program(
                &[
                    mk("f", "NOP"),
                    mk("g", "INCF 0x030,F,A"),
                    mk("h", "DECF 0x031,F,A")
                ],
                &["f", "g", "h"]
            )
        );
        let out = factor(&src, &Options::default());
        assert_eq!(
            out.matches(&format!("MOVWF {reg}")).count(),
            3,
            "{reg} moved: {out}"
        );
    }
}

#[test]
fn inline_asm_regions_are_untouched() {
    let mut fs = Vec::new();
    for (n, sep) in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"]
        .iter()
        .enumerate()
    {
        let mut s = format!("f{n}:\n    {sep}\n; --- asm start ---\n");
        for l in RUN {
            s.push_str(&format!("    {l}\n"));
        }
        s.push_str("; --- asm end ---\n    RETURN\n");
        fs.push(s);
    }
    let src = program(&fs, &["f0", "f1", "f2"]);
    assert_eq!(factor(&src, &Options::default()), src);
}

#[test]
fn interrupt_reachable_code_is_untouched() {
    let mut src = String::from("    org 0x0000\n    goto __start\n    org 0x0008\nisr:\n");
    for sep in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"] {
        src.push_str(&format!("    {sep}\n    CALL helper\n"));
        for l in RUN {
            src.push_str(&format!("    {l}\n"));
        }
    }
    src.push_str("    RETFIE\n");
    src.push_str(&func("helper", &["NOP"]));
    src.push_str("__start:\n    SLEEP\n    end\n");
    assert_eq!(factor(&src, &Options::default()), src);
}

#[test]
fn repeated_tails_merge_into_one_copy() {
    let tail = [
        "MOVF 0x020,W,A",
        "ADDWF 0x021,W,A",
        "MOVWF 0x022,A",
        "CLRF 0x023,A",
        "INCF 0x024,F,A",
    ];
    let mk = |n: &str, sep: &str| {
        let mut b = vec![sep];
        b.extend(tail);
        func(n, &b)
    };
    let src = program(
        &[
            mk("f", "NOP"),
            mk("g", "INCF 0x030,F,A"),
            mk("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let out = factor(&src, &Options::default());
    assert_eq!(out.matches("ADDWF 0x021,W,A").count(), 1, "{out}");
    let jumps = out.lines().filter(|l| {
        let t = l.trim();
        (t.starts_with("BRA ") || t.starts_with("GOTO ")) && t.contains("__pa")
    });
    assert_eq!(jumps.count(), 2, "{out}");
}

#[test]
fn bodies_never_land_before_an_org() {
    // f's end is followed by a vector org: a body there would collide.
    let mut body: Vec<&str> = Vec::new();
    for sep in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"] {
        body.push(sep);
        body.extend(RUN);
    }
    let src = format!(
        "    org 0x0000\n    goto __start\n{}    org 0x0400\n__start:\n    CALL f\n    SLEEP\n    end\n",
        func("f", &body)
    );
    let out = factor(&src, &Options::default());
    let org = out.find("org 0x0400").unwrap();
    let body_at = out.find("__pa0:").expect("still factored, far");
    assert!(body_at > org, "{out}");
    assert!(out.contains("    CALL __pa0"), "{out}");
}

#[test]
fn stack_budget_declines_rather_than_overflow() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let tight = Options {
        stack_depth: 1,
        ..Options::default()
    };
    assert_eq!(factor(&src, &tight), src);
}

#[test]
fn locs_stay_line_aligned() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let locs: Vec<Option<ir::SrcLoc>> = src
        .lines()
        .enumerate()
        .map(|(i, _)| {
            Some(ir::SrcLoc {
                file: "t.c".into(),
                line: i as u32,
                col: 1,
            })
        })
        .collect();
    let (out, out_locs) = factor_with_locs(&src, &locs, &Options::default());
    assert_eq!(out.lines().count(), out_locs.len());
    // The body's instructions carry the first site's lines.
    let lines: Vec<&str> = out.lines().collect();
    let b = lines.iter().position(|l| *l == "__pa0:").unwrap();
    let first_src = src.lines().position(|l| l.trim() == RUN[0]).unwrap();
    assert_eq!(out_locs[b + 1].as_ref().unwrap().line, first_src as u32);
}

#[test]
fn padded_twin_keeps_every_site_size() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let out = factor(
        &src,
        &Options {
            pad: true,
            ..Options::default()
        },
    );
    // Each 4-word site becomes a 2-word CALL plus 2 NOPs.
    assert_eq!(
        out.matches("    NOP\n").count() - src.matches("    NOP\n").count(),
        6,
        "{out}"
    );
    assert!(!out.contains("RCALL"), "{out}");
}

#[test]
fn output_is_deterministic() {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let a = factor(&src, &Options::default());
    for _ in 0..5 {
        assert_eq!(factor(&src, &Options::default()), a);
    }
}
