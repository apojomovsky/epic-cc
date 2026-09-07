//! The typed variable table: full `-g` metadata (`DIType`,
//! `DILocalVariable`, `DIGlobalVariable`) joined with the SSA value
//! operands each variable's `#dbg_value`/`#dbg_declare`/`#dbg_assign`
//! body record names. This is the phase-2 debugger data (epic-cc#257):
//! the driver joins it against `AllocLayout` to attach addresses; the
//! `ssa` name is the `{func}::{ssa}` local key.

use std::collections::HashMap;

use crate::{node_field, num_field, split_top_level, string_field, token_field};

/// One node of the DIType graph, keyed by metadata id in
/// [`DebugVars::types`]. References stay as ids: struct/pointer cycles
/// (`struct node { struct node* next; }`) do not resolve to a tree.
#[derive(Debug, Clone, PartialEq)]
pub enum DiTypeNode {
    Basic {
        name: String,
        size: Option<u32>,
        encoding: String,
    },
    Pointer {
        base: Option<u32>,
    },
    Typedef {
        name: Option<String>,
        base: Option<u32>,
    },
    Qualifier {
        base: Option<u32>,
    },
    Struct {
        name: Option<String>,
        size: Option<u32>,
        members: Vec<u32>,
    },
    Union {
        name: Option<String>,
        size: Option<u32>,
        members: Vec<u32>,
    },
    Enum {
        name: Option<String>,
        size: Option<u32>,
    },
    Array {
        element: Option<u32>,
        count: Option<u32>,
    },
    Member {
        name: Option<String>,
        /// Byte offset (LLVM carries bits; divided by 8 here).
        offset: Option<u32>,
        ty: Option<u32>,
    },
    Other,
}

/// One variable of the typed table: a C name, its type-graph root, and,
/// for locals, the SSA value operand its `#dbg_*` record named. `func`
/// is the defining function's name for locals, `None` for globals.
#[derive(Debug, Clone, PartialEq)]
pub struct DiVar {
    pub name: String,
    pub line: u32,
    pub ty: u32,
    pub func: Option<String>,
    pub arg: Option<u32>,
    pub ssa: Option<String>,
}

/// The parsed typed variable table. No addresses here: the join to
/// `AllocLayout.globals` (by name) and `AllocLayout.locals` (by
/// `{func}::{ssa}`) happens in the driver.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebugVars {
    pub types: HashMap<u32, DiTypeNode>,
    pub globals: Vec<DiVar>,
    pub locals: Vec<DiVar>,
}

impl DebugVars {
    /// The flat debug TYPE of a type-graph root: `int`, `enum Color`,
    /// `struct P`, `char*`, `int[4]`. Typedefs and qualifiers resolve to
    /// their base; a cycle or missing id renders as `?`.
    pub fn type_string(&self, id: u32) -> String {
        self.type_str(id, 0)
    }

    fn type_str(&self, id: u32, depth: u32) -> String {
        if depth > 32 {
            return "?".to_string();
        }
        let next = |b: &Option<u32>| -> String {
            b.map(|b| self.type_str(b, depth + 1))
                .unwrap_or_else(|| "?".to_string())
        };
        match self.types.get(&id) {
            Some(DiTypeNode::Basic { name, .. }) => name.clone(),
            Some(DiTypeNode::Pointer { base }) => format!("{}*", next(base)),
            Some(DiTypeNode::Typedef { base, .. }) | Some(DiTypeNode::Qualifier { base }) => {
                next(base)
            }
            Some(DiTypeNode::Struct { name, .. }) => {
                format!("struct {}", name.clone().unwrap_or_default())
            }
            Some(DiTypeNode::Union { name, .. }) => {
                format!("union {}", name.clone().unwrap_or_default())
            }
            Some(DiTypeNode::Enum { name, .. }) => {
                format!("enum {}", name.clone().unwrap_or_default())
            }
            Some(DiTypeNode::Array { element, count }) => {
                format!("{}[{}]", next(element), count.unwrap_or(0))
            }
            _ => "?".to_string(),
        }
    }
}

/// Composite facts collected during the scan; member/subrange tuples
/// may be defined later in the text, so composites materialize after
/// the pass.
struct RawComposite {
    id: u32,
    tag: String,
    name: Option<String>,
    size: Option<u32>,
    base: Option<u32>,
    elements: Option<u32>,
}

/// Parse every `-g` debug node in one scan, then resolve composites and
/// assemble the table. Ordering is deterministic: globals by name,
/// locals by (function, name).
pub fn parse_debug_vars(src: &str) -> DebugVars {
    let mut types: HashMap<u32, DiTypeNode> = HashMap::new();
    let mut tuples: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut subranges: HashMap<u32, u32> = HashMap::new();
    let mut composites: Vec<RawComposite> = Vec::new();
    // DILocalVariable: id, name, line, type id, arg, scope id.
    let mut local_meta: Vec<(u32, String, u32, u32, Option<u32>, u32)> = Vec::new();
    // DIGlobalVariable: id, name, line, type id, isLocal, scope id.
    let mut global_meta: Vec<(u32, String, u32, u32, bool, u32)> = Vec::new();
    let mut sub_names: HashMap<u32, String> = HashMap::new();
    // Lexical block id -> parent scope id.
    let mut scope_parent: HashMap<u32, u32> = HashMap::new();
    // Local var id -> SSA operand its body record named.
    let mut recorded: HashMap<u32, String> = HashMap::new();

    for raw in src.lines() {
        let line = raw.trim();
        if line.starts_with("#dbg_value(") || line.starts_with("#dbg_declare(") {
            record_dbg_ssa(line, &mut recorded);
            continue;
        }
        if line.starts_with("#dbg_assign(") {
            record_dbg_assign_ssa(line, &mut recorded);
            continue;
        }
        if !line.starts_with('!') {
            continue;
        }
        let Some(eq) = line.find(" = ") else {
            continue;
        };
        let Ok(id) = line[1..eq].parse::<u32>() else {
            continue;
        };
        let mut body = line[eq + 3..].trim();
        if let Some(r) = body.strip_prefix("distinct ") {
            body = r.trim();
        }
        if body.starts_with("!{") {
            let inner = &body[2..body.len().saturating_sub(1)];
            tuples.insert(
                id,
                split_top_level(inner, ',')
                    .iter()
                    .filter_map(|s| s.trim().trim_start_matches('!').parse::<u32>().ok())
                    .collect(),
            );
        } else if body.starts_with("!DIBasicType(") {
            types.insert(
                id,
                DiTypeNode::Basic {
                    name: string_field(body, "name").unwrap_or_default(),
                    size: num_field(body, "size"),
                    encoding: token_field(body, "encoding").unwrap_or_default(),
                },
            );
        } else if body.starts_with("!DIDerivedType(") {
            let tag = token_field(body, "tag").unwrap_or_default();
            let node = match tag.as_str() {
                "DW_TAG_pointer_type" => DiTypeNode::Pointer {
                    base: node_field(body, "baseType"),
                },
                "DW_TAG_member" => DiTypeNode::Member {
                    name: string_field(body, "name"),
                    offset: num_field(body, "offset").map(|bits| bits / 8),
                    ty: node_field(body, "baseType"),
                },
                "DW_TAG_typedef" => DiTypeNode::Typedef {
                    name: string_field(body, "name"),
                    base: node_field(body, "baseType"),
                },
                "DW_TAG_const_type"
                | "DW_TAG_volatile_type"
                | "DW_TAG_restrict_type"
                | "DW_TAG_atomic_type" => DiTypeNode::Qualifier {
                    base: node_field(body, "baseType"),
                },
                _ => DiTypeNode::Other,
            };
            types.insert(id, node);
        } else if body.starts_with("!DICompositeType(") {
            composites.push(RawComposite {
                id,
                tag: token_field(body, "tag").unwrap_or_default(),
                name: string_field(body, "name"),
                size: num_field(body, "size"),
                base: node_field(body, "baseType"),
                elements: node_field(body, "elements"),
            });
        } else if body.starts_with("!DISubrange(") {
            subranges.insert(id, num_field(body, "count").unwrap_or(0));
        } else if body.starts_with("!DILocalVariable(") {
            if let (Some(name), Some(scope)) =
                (string_field(body, "name"), node_field(body, "scope"))
            {
                local_meta.push((
                    id,
                    name,
                    num_field(body, "line").unwrap_or(0),
                    node_field(body, "type").unwrap_or(0),
                    num_field(body, "arg"),
                    scope,
                ));
            }
        } else if body.starts_with("!DISubprogram(") {
            if let Some(name) = string_field(body, "name") {
                sub_names.insert(id, name);
            }
        } else if body.starts_with("!DILexicalBlock(") || body.starts_with("!DILexicalBlockFile(") {
            if let Some(parent) = node_field(body, "scope") {
                scope_parent.insert(id, parent);
            }
        } else if body.starts_with("!DIGlobalVariable(") {
            if let Some(name) = string_field(body, "name") {
                let is_local = token_field(body, "isLocal").as_deref() == Some("true");
                global_meta.push((
                    id,
                    name,
                    num_field(body, "line").unwrap_or(0),
                    node_field(body, "type").unwrap_or(0),
                    is_local,
                    node_field(body, "scope").unwrap_or(0),
                ));
            }
        }
    }

    for c in composites {
        let members = || {
            c.elements
                .and_then(|e| tuples.get(&e).cloned())
                .unwrap_or_default()
        };
        let node = match c.tag.as_str() {
            "DW_TAG_structure_type" => DiTypeNode::Struct {
                name: c.name,
                size: c.size,
                members: members(),
            },
            "DW_TAG_union_type" => DiTypeNode::Union {
                name: c.name,
                size: c.size,
                members: members(),
            },
            "DW_TAG_enumeration_type" => DiTypeNode::Enum {
                name: c.name,
                size: c.size,
            },
            "DW_TAG_array_type" => DiTypeNode::Array {
                element: c.base,
                count: c.elements.and_then(|e| {
                    tuples
                        .get(&e)
                        .and_then(|ids| ids.iter().find_map(|i| subranges.get(i).copied()))
                }),
            },
            _ => DiTypeNode::Other,
        };
        types.insert(c.id, node);
    }

    // A function-local static (`isLocal: true`) is a global whose C
    // symbol clang names `{func}.{name}`; its `func` records that, so
    // the driver joins the sanitized `{func}_{name}` symbol.
    let mut globals: Vec<DiVar> = global_meta
        .into_iter()
        .map(|(_id, name, line, ty, is_local, scope)| DiVar {
            name,
            line,
            ty,
            func: is_local
                .then(|| scope_func(Some(scope), &scope_parent, &sub_names))
                .flatten(),
            arg: None,
            ssa: None,
        })
        .collect();
    let mut locals: Vec<DiVar> = local_meta
        .into_iter()
        .map(|(id, name, line, ty, arg, scope)| DiVar {
            name,
            line,
            ty,
            func: scope_func(Some(scope), &scope_parent, &sub_names),
            arg,
            ssa: recorded.get(&id).cloned(),
        })
        .collect();
    globals.sort_by(|a, b| a.name.cmp(&b.name));
    locals.sort_by(|a, b| {
        (a.func.as_deref(), a.name.as_str()).cmp(&(b.func.as_deref(), b.name.as_str()))
    });
    DebugVars {
        types,
        globals,
        locals,
    }
}

/// Walk a scope chain (possibly through nested lexical blocks) to its
/// subprogram's function name. Bounded: a malformed cyclic chain gives
/// up instead of looping.
fn scope_func(
    scope: Option<u32>,
    parents: &HashMap<u32, u32>,
    subs: &HashMap<u32, String>,
) -> Option<String> {
    let mut s = scope?;
    for _ in 0..64 {
        if let Some(n) = subs.get(&s) {
            return Some(n.clone());
        }
        s = *parents.get(&s)?;
    }
    None
}

/// Capture `#dbg_value(VALUE, !VAR, ...)` / `#dbg_declare(...)`'s SSA
/// operand. Constants, `undef`, `null` and `poison` map nothing.
fn record_dbg_ssa(line: &str, recorded: &mut HashMap<u32, String>) {
    let Some(args) = dbg_args(line) else {
        return;
    };
    if args.len() < 2 {
        return;
    }
    if let (Some(var), Some(ssa)) = (dbg_var_id(&args[1]), ssa_of(&args[0])) {
        recorded.entry(var).or_insert(ssa);
    }
}

/// `#dbg_assign(VALUE, !VAR, !EXPR, !ID, ADDR, !EXPR, !LOC)`: the value
/// is often `undef` for aggregates, the address operand (arg 4) then
/// names the storage.
fn record_dbg_assign_ssa(line: &str, recorded: &mut HashMap<u32, String>) {
    let Some(args) = dbg_args(line) else {
        return;
    };
    if args.len() < 5 {
        return;
    }
    let Some(var) = dbg_var_id(&args[1]) else {
        return;
    };
    if let Some(ssa) = ssa_of(&args[0]).or_else(|| ssa_of(&args[4])) {
        recorded.entry(var).or_insert(ssa);
    }
}

/// The top-level comma args between a record's outer parens.
fn dbg_args(line: &str) -> Option<Vec<String>> {
    let open = line.find('(')?;
    let inner = &line[open + 1..line.len().saturating_sub(1)];
    Some(
        split_top_level(inner, ',')
            .iter()
            .map(|s| s.trim().to_string())
            .collect(),
    )
}

/// The `!N` DILocalVariable id of a record's variable operand.
fn dbg_var_id(arg: &str) -> Option<u32> {
    arg.trim().trim_start_matches('!').parse().ok()
}

/// `%N` from a typed value operand like `i16 %3`; `None` for constants.
fn ssa_of(arg: &str) -> Option<String> {
    arg.split_whitespace()
        .find_map(|t| t.strip_prefix('%').map(|s| s.to_string()))
}
