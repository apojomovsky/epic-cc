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

/// A three-function program (one shared run) with `data` between
/// `__start` and `end`, the layout isel emits for flash consts.
fn program_with_data(data: &str) -> String {
    let src = program(
        &[
            with_run("f", "NOP"),
            with_run("g", "INCF 0x030,F,A"),
            with_run("h", "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    src.replace("    end\n", &format!("{data}    end\n"))
}

#[test]
fn odd_trailing_data_is_padded_before_far_bodies() {
    // The assembler packs `db` bytes unpadded, so a far body
    // placed after an odd run would start mid-word and OR the last data
    // byte into its first opcode.
    let out = factor(
        &program_with_data("blob:\n    db 0x01, 0x02\n    db 0x03\n"),
        &Options::default(),
    );
    let pad = out.find("    db 0x00").expect("pad byte");
    let body = out.find("__pa0:").unwrap();
    assert!(out.find("db 0x03").unwrap() < pad && pad < body, "{out}");
}

#[test]
fn even_trailing_data_gets_no_pad() {
    let out = factor(
        &program_with_data("blob:\n    db 0x01, 0x02\n"),
        &Options::default(),
    );
    assert!(out.contains("__pa0:"), "{out}");
    assert!(!out.contains("db 0x00"), "{out}");
}

#[test]
fn odd_trailing_data_is_padded_without_end() {
    let src = program_with_data("blob:\n    db 0x01, 0x02\n    db 0x03\n")
        .replace("    SLEEP\n    end\n", "    SLEEP\n");
    let out = factor(&src, &Options::default());
    let pad = out.find("    db 0x00").expect("pad byte");
    let body = out.find("__pa0:").unwrap();
    assert!(out.find("db 0x03").unwrap() < pad && pad < body, "{out}");
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
    // The backend emits asm unindented, one line per template line.
    let mut fs = Vec::new();
    for (n, sep) in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"]
        .iter()
        .enumerate()
    {
        let mut s = format!("f{n}:\n    {sep}\n; --- asm start ---\n");
        for l in RUN {
            s.push_str(&format!("{l}\n"));
        }
        s.push_str("; --- asm end ---\n    RETURN\n");
        fs.push(s);
    }
    let src = program(&fs, &["f0", "f1", "f2"]);
    assert_eq!(factor(&src, &Options::default()), src);
}

/// Three functions whose compiled run follows an asm block that ends in
/// `asm_tail`; the run repeats, so only the skip rules keep it inline.
fn after_asm(asm_tail: &str) -> String {
    let mut fs = Vec::new();
    for (n, sep) in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"]
        .iter()
        .enumerate()
    {
        let mut s =
            format!("f{n}:\n    {sep}\n; --- asm start ---\n{asm_tail}\n; --- asm end ---\n");
        for l in RUN {
            s.push_str(&format!("    {l}\n"));
        }
        s.push_str("    RETURN\n");
        fs.push(s);
    }
    program(&fs, &["f0", "f1", "f2"])
}

fn first_after_asm_kept(out: &str) {
    let lines: Vec<&str> = out.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.starts_with("; --- asm end ---") {
            assert_eq!(lines[i + 1].trim(), RUN[0], "skip successor moved: {out}");
        }
    }
}

#[test]
fn unindented_asm_skip_protects_its_successor() {
    let src = after_asm("btfss 0x80, 0");
    first_after_asm_kept(&factor(&src, &Options::default()));
}

#[test]
fn labelled_asm_skip_protects_its_successor() {
    let src = after_asm("here: BTFSC 0x80, 0");
    first_after_asm_kept(&factor(&src, &Options::default()));
}

#[test]
fn asm_ending_in_data_protects_its_successor() {
    // A raw data word could be any instruction, a skip included.
    let src = after_asm("dw 0xA4A0");
    first_after_asm_kept(&factor(&src, &Options::default()));
}

#[test]
fn asm_ending_in_a_plain_instruction_leaves_its_successor_free() {
    let src = after_asm("sleep");
    let out = factor(&src, &Options::default());
    assert!(out.contains("__pa0"), "{out}");
}

#[test]
fn asm_calls_count_toward_the_stack_budget() {
    // f's asm calls g: with the edge, __start->f->g is two return
    // addresses plus the new leaf, which a 2-entry stack cannot hold
    // (without the edge the pass would think 2 suffice).
    let mut fs = vec![func("g", &["NOP"])];
    for (n, sep) in ["NOP", "INCF 0x030,F,A", "DECF 0x031,F,A"]
        .iter()
        .enumerate()
    {
        let mut s = format!(
            "f{n}:\n    {sep}\n; --- asm start ---\nx{n}: call g\n; --- asm end ---\n    NOP\n"
        );
        for l in RUN {
            s.push_str(&format!("    {l}\n"));
        }
        s.push_str("    RETURN\n");
        fs.push(s);
    }
    let src = program(&fs, &["f0", "f1", "f2"]);
    let tight = Options {
        stack_depth: 2,
        ..Options::default()
    };
    assert_eq!(factor(&src, &tight), src);
    let roomy = Options {
        stack_depth: 3,
        ..Options::default()
    };
    assert_ne!(factor(&src, &roomy), src);
}

#[test]
fn priority_handlers_stack_in_the_budget() {
    // Two vectors, each calling a helper: high preempts low, so the
    // interrupt side needs both chains, 2 + 2, on top of main's 1 + leaf.
    let mut src = String::from(
        "    org 0x0000\n    goto __start\n    org 0x0008\n    goto hi\n    org 0x0018\n    goto lo\n",
    );
    src.push_str("hi:\n    CALL h1\n    RETFIE\nlo:\n    CALL h2\n    RETFIE\n");
    src.push_str(&func("h1", &["NOP"]));
    src.push_str(&func("h2", &["NOP"]));
    for f in [
        with_run("f", "NOP"),
        with_run("g", "INCF 0x030,F,A"),
        with_run("h", "DECF 0x031,F,A"),
    ] {
        src.push_str(&f);
    }
    src.push_str("__start:\n    CALL f\n    CALL g\n    CALL h\n    SLEEP\n    end\n");
    let at = |d| Options {
        stack_depth: d,
        ..Options::default()
    };
    assert_eq!(
        factor(&src, &at(5)),
        src,
        "2 (main+leaf) + 2 + 2 does not fit 5"
    );
    assert_ne!(factor(&src, &at(6)), src);
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

#[test]
fn spot_replaced_by_a_tail_merge_still_hosts_local_bodies() {
    // f comes last, so its tail is merged into g's kept copy and its final
    // RETURN disappears; f's local body must follow the GOTO that replaced
    // it, still ahead of the next label.
    let tail = [
        "MOVF 0x040,W,A",
        "ADDWF 0x041,W,A",
        "MOVWF 0x042,A",
        "CLRF 0x043,A",
        "INCF 0x044,F,A",
    ];
    let mut g = vec!["NOP"];
    g.extend(tail);
    let mut h = vec!["INCF 0x030,F,A"];
    h.extend(tail);
    let mut f: Vec<&str> = Vec::new();
    for sep in ["NOP", "INCF 0x031,F,A", "DECF 0x032,F,A"] {
        f.push(sep);
        f.extend(RUN);
    }
    f.push("DECF 0x033,F,A");
    f.extend(tail);
    let src = format!(
        "{}next:\n    NOP\n    RETURN\n",
        program(
            &[func("g", &g), func("h", &h), func("f", &f)],
            &["g", "h", "f", "next"]
        )
        .replace("__start:", "PLACEHOLDER:")
    )
    .replace("PLACEHOLDER:", "__start:");
    let out = factor(&src, &Options::default());
    let lines: Vec<&str> = out.lines().collect();
    let body = lines
        .iter()
        .position(|l| {
            l.starts_with("__pa")
                && lines
                    .iter()
                    .any(|x| x.trim() == format!("RCALL {}", l.trim_end_matches(':')))
        })
        .expect("a local RCALL body");
    let before = lines[..body]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap()
        .trim();
    assert!(
        before.starts_with("GOTO __pa") || before.starts_with("BRA __pa"),
        "local body must follow the tail-merge jump that replaced f's RETURN, got {before:?}:\n{out}"
    );
}

/// Flash words in a factored listing: two-word forms count double, layout
/// directives do not count.
fn words(out: &str) -> i64 {
    let mut n = 0i64;
    for l in out.lines() {
        if !l.starts_with(' ') && !l.starts_with('\t') {
            continue;
        }
        let m = l
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if m.is_empty() || ["ORG", "END", "LIST", "RADIX", "DB", "DW", "DATA"].contains(&m.as_str())
        {
            continue;
        }
        n += if ["MOVFF", "LFSR", "GOTO", "CALL", "MOVSF", "MOVSS"].contains(&m.as_str()) {
            2
        } else {
            1
        };
    }
    n
}

/// A six-instruction run shared whole by three functions.
const CANARY_RUN: [&str; 6] = [
    "MOVF 0x020,W,A",
    "ADDWF 0x021,W,A",
    "MOVWF 0x022,A",
    "CLRF 0x023,A",
    "INCF 0x024,F,A",
    "DECF 0x025,F,A",
];

#[test]
fn one_byte_shift_with_local_shape_change_stays_within_cap() {
    // A narrowing-style perturbation: every address at or above 0x021
    // slides down one byte, and one site's word store becomes a byte
    // clear. The two untouched sites must keep sharing one body, and
    // the factored size must not move more than the #306 row cap.
    let slid: Vec<String> = CANARY_RUN
        .iter()
        .map(|l| {
            // Single pass: sequential replaces would cascade (0x022
            // becomes 0x021, then matches the 0x021 rule).
            let mut t = String::new();
            let mut rest = *l;
            while let Some(p) = rest.find("0x") {
                t.push_str(&rest[..p]);
                let hex: String = rest[p + 2..]
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .collect();
                let v = u32::from_str_radix(&hex, 16).unwrap();
                if v >= 0x021 {
                    let w = v - 1;
                    t.push_str(&format!("0x{w:03X}"));
                } else {
                    t.push_str(&rest[p..p + 2 + hex.len()]);
                }
                rest = &rest[p + 2 + hex.len()..];
            }
            t.push_str(rest);
            t
        })
        .collect();
    let slid_run: Vec<&str> = slid.iter().map(String::as_str).collect();
    let mut shaped = slid.clone();
    shaped[2] = "CLRF 0x021,A".to_string();
    let shaped_run: Vec<&str> = shaped.iter().map(String::as_str).collect();
    let mk = |name: &str, run: &[&str], sep: &str| {
        let mut b = vec![sep];
        b.extend(run.iter().copied());
        b.push(sep);
        func(name, &b)
    };
    let base = program(
        &[
            mk("f", &CANARY_RUN, "NOP"),
            mk("g", &CANARY_RUN, "INCF 0x030,F,A"),
            mk("h", &CANARY_RUN, "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let shifted = program(
        &[
            mk("f", &slid_run, "NOP"),
            mk("g", &slid_run, "INCF 0x030,F,A"),
            mk("h", &shaped_run, "DECF 0x031,F,A"),
        ],
        &["f", "g", "h"],
    );
    let out_base = factor(&base, &Options::default());
    let out_shifted = factor(&shifted, &Options::default());
    // The shared body holds the whole run, not a subrun fragment: a
    // re-partitioning selector would split or drop it.
    fn body_lines<'a>(out: &'a str, marker: &str) -> Vec<&'a str> {
        let lines: Vec<&str> = out.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if (l.starts_with(' ') || l.starts_with('\t'))
                || !l.trim_end_matches(':').starts_with("__pa")
            {
                continue;
            }
            let body: Vec<&str> = lines[i + 1..i + 7].iter().map(|x| x.trim()).collect();
            if body.contains(&marker) {
                return body;
            }
        }
        panic!("no body holds {marker}:\n{out}");
    }
    assert_eq!(
        body_lines(&out_base, "MOVWF 0x022,A"),
        CANARY_RUN.to_vec(),
        "base shares the whole run:\n{out_base}"
    );
    assert_eq!(
        body_lines(&out_shifted, "MOVWF 0x021,A"),
        slid_run,
        "untouched sites keep sharing the whole slid run:\n{out_shifted}"
    );
    let delta = words(&out_shifted) - words(&out_base);
    assert!(
        delta.abs() <= 8,
        "one-byte shift moved factoring by {delta} words:\n{out_shifted}"
    );
}
