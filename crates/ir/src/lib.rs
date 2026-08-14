//! Canonical IR text format shared by all pipeline stages. Text boundary:
//! every stage reads IR text in and writes IR text out.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty { I1, I8, I16 }
impl Ty {
    pub fn bytes(self) -> u8 { match self { Ty::I1 | Ty::I8 => 1, Ty::I16 => 2 } }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Val { Reg(String), Const(i64), Global(String) }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp { Add, Sub, And, Or, Xor }

#[derive(Clone, Debug)]
pub struct Load { pub dst: String, pub ty: Ty, pub ptr: String } // ptr = "@name" or "%name"
#[derive(Clone, Debug)]
pub struct Store { pub ty: Ty, pub val: Val, pub ptr: String }
#[derive(Clone, Debug)]
pub struct Bin { pub dst: String, pub op: BinOp, pub ty: Ty, pub a: Val, pub b: Val }

#[derive(Clone, Debug)]
pub enum Inst {
    Load(Load),
    Store(Store),
    Bin(Bin),
    Ret(Option<(Ty, Val)>),
}

#[derive(Clone, Debug)]
pub struct Block { pub label: String, pub insts: Vec<Inst> }

#[derive(Clone, Debug)]
pub struct Func { pub name: String, pub ret: Option<Ty>, pub params: Vec<(Ty, String)>, pub blocks: Vec<Block> }

#[derive(Clone, Debug)]
pub struct Global { pub name: String, pub ty: Ty, pub is_const: bool, pub addr: Option<u8> }

#[derive(Clone, Debug)]
pub struct Module { pub globals: Vec<Global>, pub funcs: Vec<Func> }

fn val_str(v: &Val) -> String {
    match v { Val::Reg(r) => format!("%{r}"), Val::Const(k) => k.to_string(), Val::Global(g) => format!("@{g}") }
}

pub fn serialize(m: &Module) -> String {
    let mut out = String::new();
    for g in &m.globals {
        let kind = if g.is_const { "const" } else { "global" };
        match g.addr { Some(a) => out.push_str(&format!("{kind} {} {} @0x{a:02X}\n", g.name, ty_str(g.ty))), None => out.push_str(&format!("{kind} {} {}\n", g.name, ty_str(g.ty))) }
    }
    for f in &m.funcs {
        let params: Vec<String> = f.params.iter().map(|(t, n)| format!("{t:?} %{n}").replace("I8", "i8").replace("I16", "i16").replace("I1", "i1")).collect();
        let ret = match f.ret { Some(t) => ty_str(t), None => "void".to_string() };
        out.push_str(&format!("fn {}({}) -> {ret}\n", f.name, params.join(", ")));
        for b in &f.blocks {
            out.push_str(&format!("  block {}:\n", b.label));
            for i in &b.insts {
                out.push_str(&format!("    {}\n", inst_str(i)));
            }
        }
    }
    out
}

fn ty_str(t: Ty) -> String { match t { Ty::I1 => "i1".into(), Ty::I8 => "i8".into(), Ty::I16 => "i16".into() } }

fn inst_str(i: &Inst) -> String {
    match i {
        Inst::Load(l) => format!("%{} = load {} {}", l.dst, ty_str(l.ty), l.ptr),
        Inst::Store(s) => format!("store {} {} {}", ty_str(s.ty), val_str(&s.val), s.ptr),
        Inst::Bin(b) => format!("%{} = {} {} {} {}", b.dst, op_str(b.op), ty_str(b.ty), val_str(&b.a), val_str(&b.b)),
        Inst::Ret(None) => "ret void".into(),
        Inst::Ret(Some((t, v))) => format!("ret {} {}", ty_str(*t), val_str(v)),
    }
}

fn op_str(o: BinOp) -> &'static str { match o { BinOp::Add => "add", BinOp::Sub => "sub", BinOp::And => "and", BinOp::Or => "or", BinOp::Xor => "xor" } }

pub fn parse(text: &str) -> Module {
    let mut globals = Vec::new();
    let mut funcs = Vec::new();
    let mut cur_func: Option<Func> = None;
    let mut cur_block: Option<Block> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some(rest) = line.strip_prefix("global ").or_else(|| line.strip_prefix("const ")) {
            let is_const = line.starts_with("const ");
            let parts: Vec<&str> = rest.split_whitespace().collect();
            // format: <name> <ty> [@addr]
            let name = parts[0].to_string();
            let ty = parse_ty(parts[1]);
            let addr = if parts.len() >= 3 { parse_addr(parts[2]) } else { None };
            globals.push(Global { name, ty, is_const, addr });
        } else if line.starts_with("fn ") {
            let rest = &line[3..];
            let open = rest.find('(').unwrap();
            let name = rest[..open].trim().to_string();
            let close = rest.rfind(')').unwrap();
            let sig = &rest[open + 1..close];
            let ret = rest[close + 1..].trim().trim_start_matches("->").trim();
            let params = if sig.trim().is_empty() { vec![] } else {
                sig.split(',').map(|p| { let mut it = p.trim().split_whitespace(); let t = parse_ty(it.next().unwrap()); let n = it.next().unwrap().trim_start_matches('%').to_string(); (t, n) }).collect()
            };
            if let Some(f) = cur_func.as_mut() { if let Some(b) = cur_block.take() { f.blocks.push(b); } }
            if let Some(f) = cur_func.take() { funcs.push(f); }
            cur_func = Some(Func { name, ret: if ret == "void" { None } else { Some(parse_ty(ret)) }, params, blocks: Vec::new() });
        } else if line.starts_with("block ") {
            if let Some(f) = cur_func.as_mut() { if let Some(b) = cur_block.take() { f.blocks.push(b); } }
            let label = line["block ".len()..].trim_end_matches(':').to_string();
            cur_block = Some(Block { label, insts: Vec::new() });
        } else {
            let inst = parse_inst(line);
            match (&mut cur_func, &mut cur_block) {
                (Some(_), Some(b)) => b.insts.push(inst),
                (Some(_), None) => panic!("instruction before any block: {line}"),
                (None, _) => panic!("instruction outside a function: {line}"),
            }
        }
    }
    if let Some(f) = cur_func.as_mut() { if let Some(b) = cur_block.take() { f.blocks.push(b); } }
    if let Some(f) = cur_func.take() { funcs.push(f); }
    Module { globals, funcs }
}

fn parse_ty(s: &str) -> Ty { match s { "i1" => Ty::I1, "i8" => Ty::I8, "i16" => Ty::I16, other => panic!("unsupported type {other}") } }
fn parse_addr(s: &str) -> Option<u8> { s.strip_prefix('@').map(|h| u8::from_str_radix(h.trim_start_matches("0x"), 16).unwrap()) }
fn parse_val(s: &str) -> Val {
    let s = s.trim_end_matches(',');
    if let Some(r) = s.strip_prefix('%') { Val::Reg(r.to_string()) }
    else if let Some(g) = s.strip_prefix('@') { Val::Global(g.to_string()) }
    else { Val::Const(s.parse().unwrap_or_else(|_| panic!("bad value {s}"))) }
}
fn parse_inst(line: &str) -> Inst {
    if let Some(rest) = line.strip_prefix("store ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        return Inst::Store(Store { ty: parse_ty(parts[0]), val: parse_val(parts[1]), ptr: parts[2].to_string() });
    }
    if let Some(rest) = line.strip_prefix("ret ") {
        if rest == "void" { return Inst::Ret(None); }
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        return Inst::Ret(Some((t, parse_val(it.next().unwrap()))));
    }
    // defining instruction: %d = op ...
    let eq = line.find(" = ").unwrap();
    let dst = line[..eq].trim_start_matches('%').to_string();
    let body = line[eq + 3..].trim();
    if let Some(rest) = body.strip_prefix("load ") {
        let mut it = rest.split_whitespace();
        let t = parse_ty(it.next().unwrap());
        let ptr = it.next().unwrap().to_string();
        return Inst::Load(Load { dst, ty: t, ptr });
    }
    let mut it = body.split_whitespace();
    let op = it.next().unwrap();
    let t = parse_ty(it.next().unwrap());
    let a = parse_val(it.next().unwrap());
    let b = parse_val(it.next().unwrap());
    let op = match op { "add" => BinOp::Add, "sub" => BinOp::Sub, "and" => BinOp::And, "or" => BinOp::Or, "xor" => BinOp::Xor, other => panic!("unsupported op {other}") };
    Inst::Bin(Bin { dst, op, ty: t, a, b })
}
