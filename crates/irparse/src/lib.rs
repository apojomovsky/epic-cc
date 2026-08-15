//! Parser for LLVM IR text (`.ll`) into the canonical `ir::Module`.
//!
//! Supports the milestone-2 integer-spine subset the PIC8 backend consumes:
//! `load`/`store` (global and SSA pointer operands), `add`/`sub`/`and`/`or`/
//! `xor`, `ret`, `zext`/`trunc`, `icmp`, `select`, `br`/`brcond`, `call`, and
//! `phi`. Any other opcode, or any structurally malformed input, panics loudly
//! rather than silently misparsing.

use ir::{Bin, BinOp, Block, Br, BrCond, Call, Func, Global, Icmp, Inst, Load, Module, Phi, Select, Store, Trunc, Ty, Val, Zext};

/// Strip LLVM parameter/return attributes we do not model, e.g.
/// `i16 noundef range(i16 -32768, 255) %1` -> `i16 %1`.
fn strip_attrs(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for tok in s.split_whitespace() {
        if depth > 0 {
            depth += tok.matches('(').count();
            depth -= tok.matches(')').count();
            continue;
        }
        if tok.starts_with("range(") || tok.starts_with("align(") {
            depth = 1 + tok.matches('(').count() - 1 - tok.matches(')').count();
            continue;
        }
        match tok {
            "noundef" | "nsw" | "nuw" | "nneg" | "volatile" | "tail" | "fastcc" | "inbounds"
            | "dso_local" | "local_unnamed_addr" | "internal" | "unnamed_addr" | "zeroext"
            | "signext" | "disjoint" => continue,
            _ => {}
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }
    out
}

fn ty_of(s: &str) -> Ty {
    match s {
        "i1" => Ty::I1,
        "i8" => Ty::I8,
        "i16" => Ty::I16,
        other => panic!("SPIKE: unsupported type {other:?}"),
    }
}

fn parse_val(s: &str) -> Val {
    let s = s.trim().trim_end_matches(',');
    if let Some(r) = s.strip_prefix('%') {
        Val::Reg(r.to_string())
    } else if let Some(g) = s.strip_prefix('@') {
        Val::Global(g.to_string())
    } else if s == "true" {
        Val::Const(1)
    } else if s == "false" {
        Val::Const(0)
    } else {
        Val::Const(s.parse::<i64>().unwrap_or_else(|_| panic!("SPIKE: cannot parse value {s:?}")))
    }
}

/// A pointer operand on load/store: `"@name"` (global) or `"%name"` (SSA reg).
fn parse_ptr(s: &str) -> String {
    let tok = s.split_whitespace().nth(1).unwrap();
    if tok.starts_with('@') {
        tok.to_string()
    } else {
        format!("%{}", tok.trim_start_matches('%'))
    }
}

/// Parse `.ll` text into canonical IR.
pub fn parse_ll(src: &str) -> Module {
    let mut globals = Vec::new();
    let mut funcs = Vec::new();
    let mut lines = src.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('!') {
            continue;
        }

        // Global definitions: "@name = ... global|constant <ty> ..."
        if line.starts_with('@') {
            let eq = line.find('=').unwrap();
            let name = line[1..eq].trim().to_string();
            let after = line[eq + 1..].trim();
            let (is_const, rest) = if let Some(i) = after.find("global ") {
                (false, &after[i + "global ".len()..])
            } else if let Some(i) = after.find("constant ") {
                (true, &after[i + "constant ".len()..])
            } else {
                continue; // not a global definition we care about
            };
            let rest = rest.trim();
            // scalar global: "<ty> <init>[, align N]" -> type is the first token
            let ty = ty_of(rest.split_whitespace().next().unwrap());
            globals.push(Global { name, ty, is_const, addr: None });
            continue;
        }

        if line.starts_with("define") {
            let head = strip_attrs(line);
            let ret_str = head.split_whitespace().nth(1).unwrap().to_string();
            let ret = if ret_str == "void" { None } else { Some(ty_of(&ret_str)) };
            let name = head[head.find('@').unwrap() + 1..head.find('(').unwrap()].to_string();
            let params_str = &head[head.find('(').unwrap() + 1..head.rfind(')').unwrap()];
            let mut params = Vec::new();
            for p in params_str.split(',') {
                let p = strip_attrs(p);
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                let mut it = p.split_whitespace();
                let t = ty_of(it.next().unwrap());
                let n = it.next().unwrap().trim_start_matches('%').to_string();
                params.push((t, n));
            }

            let mut blocks: Vec<Block> = vec![Block { label: "0".into(), insts: Vec::new() }];
            for raw in lines.by_ref() {
                let l = raw.trim();
                if l == "}" {
                    break;
                }
                if l.is_empty() || l.starts_with(';') {
                    continue;
                }
                if let Some(colon) = l.find(':') {
                    let head = &l[..colon];
                    if !head.is_empty()
                        && head.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                        && !l.starts_with('%')
                    {
                        blocks.push(Block { label: head.to_string(), insts: Vec::new() });
                        continue;
                    }
                }
                let inst = parse_inst(l);
                blocks.last_mut().unwrap().insts.push(inst);
            }
            funcs.push(Func { name, ret, params, blocks });
        }
    }
    Module { globals, funcs }
}

fn parse_inst(line: &str) -> Inst {
    // Drop trailing metadata: ", !tbaa !2" / ", !llvm.loop !5"
    let line = match line.find(", !") {
        Some(i) => &line[..i],
        None => line,
    };
    let line = strip_attrs(line);
    let line = line.trim();

    let (dst, rest) = match line.find(" = ") {
        Some(i) => (Some(line[..i].trim_start_matches('%').to_string()), line[i + 3..].trim()),
        None => (None, line),
    };

    let op = rest.split_whitespace().next().unwrap();
    match op {
        "load" => {
            let args: Vec<&str> = rest["load".len()..].split(',').map(|s| s.trim()).collect();
            let ty = ty_of(args[0]);
            Inst::Load(Load { dst: dst.unwrap(), ty, ptr: parse_ptr(args[1]) })
        }
        "store" => {
            let args: Vec<&str> = rest["store".len()..].split(',').map(|s| s.trim()).collect();
            let mut it = args[0].split_whitespace();
            let ty = ty_of(it.next().unwrap());
            let val = parse_val(it.next().unwrap());
            Inst::Store(Store { ty, val, ptr: parse_ptr(args[1]) })
        }
        "add" | "and" | "or" | "xor" | "sub" => {
            let body = rest[op.len()..].trim();
            let mut it = body.split_whitespace();
            let ty = ty_of(it.next().unwrap());
            let a = parse_val(it.next().unwrap());
            let b = parse_val(it.next().unwrap());
            let o = match op {
                "add" => BinOp::Add,
                "and" => BinOp::And,
                "or" => BinOp::Or,
                "xor" => BinOp::Xor,
                _ => BinOp::Sub,
            };
            Inst::Bin(Bin { dst: dst.unwrap(), op: o, ty, a, b })
        }
        "zext" | "trunc" => {
            let body = &rest[op.len()..];
            let to_i = body.rfind(" to ").unwrap();
            let (lhs, rhs) = (body[..to_i].trim(), body[to_i + 4..].trim());
            let mut it = lhs.split_whitespace();
            let from = ty_of(it.next().unwrap());
            let val = parse_val(it.next().unwrap());
            let to = ty_of(rhs);
            if op == "zext" {
                Inst::Zext(Zext { dst: dst.unwrap(), from, val, to })
            } else {
                Inst::Trunc(Trunc { dst: dst.unwrap(), from, val, to })
            }
        }
        "icmp" => {
            let body = rest["icmp".len()..].trim();
            let mut it = body.split_whitespace();
            let pred = it.next().unwrap().to_string();
            let ty = ty_of(it.next().unwrap());
            let a = parse_val(it.next().unwrap());
            let b = parse_val(it.next().unwrap());
            Inst::Icmp(Icmp { dst: dst.unwrap(), pred, ty, a, b })
        }
        "select" => {
            let body = rest["select".len()..].trim();
            let parts: Vec<&str> = body.split(',').map(|s| s.trim()).collect();
            let cond = parse_val(parts[0].split_whitespace().nth(1).unwrap());
            let mut it1 = parts[1].split_whitespace();
            let ty = ty_of(it1.next().unwrap());
            let a = parse_val(it1.next().unwrap());
            let b = parse_val(parts[2].split_whitespace().nth(1).unwrap());
            Inst::Select(Select { dst: dst.unwrap(), cond, ty, a, b })
        }
        "call" => {
            let body = rest["call".len()..].trim();
            let paren = body.find('(').unwrap();
            let head = &body[..paren];
            let mut it = head.split_whitespace();
            let ty_tok = it.next().unwrap();
            let ty = if ty_tok == "void" { None } else { Some(ty_of(ty_tok)) };
            let func = head[head.find('@').unwrap() + 1..].trim().to_string();
            let args_str = &body[paren + 1..body.rfind(')').unwrap()];
            let mut args = Vec::new();
            for a in args_str.split(',') {
                let a = a.trim();
                if a.is_empty() {
                    continue;
                }
                let mut it = a.split_whitespace();
                let t = ty_of(it.next().unwrap());
                args.push((t, parse_val(it.next().unwrap())));
            }
            Inst::Call(Call { dst, ty, func, args })
        }
        "br" => {
            let body = rest["br".len()..].trim();
            if body.starts_with("label") {
                Inst::Br(Br { target: body.split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string() })
            } else {
                let parts: Vec<&str> = body.split(',').map(|s| s.trim()).collect();
                let cond = parse_val(parts[0].split_whitespace().nth(1).unwrap());
                let t = parts[1].split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string();
                let f = parts[2].split_whitespace().nth(1).unwrap().trim_start_matches('%').to_string();
                Inst::BrCond(BrCond { cond, t, f })
            }
        }
        "phi" => {
            let body = rest["phi".len()..].trim();
            let ty = ty_of(body.split_whitespace().next().unwrap());
            let mut incoming = Vec::new();
            for part in body.split('[').skip(1) {
                let inner = part.split(']').next().unwrap();
                let mut it = inner.split(',');
                let v = parse_val(it.next().unwrap());
                let lbl = it.next().unwrap().trim().trim_start_matches('%').to_string();
                incoming.push((v, lbl));
            }
            Inst::Phi(Phi { dst: dst.unwrap(), ty, incoming })
        }
        "ret" => {
            let body = rest["ret".len()..].trim();
            if body == "void" {
                Inst::Ret(None)
            } else {
                let mut it = body.split_whitespace();
                let ty = ty_of(it.next().unwrap());
                Inst::Ret(Some((ty, parse_val(it.next().unwrap()))))
            }
        }
        other => panic!("SPIKE LIMIT: unsupported for milestone 1: opcode {other:?} in line: {line}"),
    }
}
