//! Call graph and stack-depth check boundary for the PIC8 pipeline.

use std::collections::HashMap;

use ir::{Inst, Module};

pub struct CallGraph {
    pub edges: Vec<(String, String)>,
    pub max_depth: usize,
}

pub fn build(m: &Module) -> CallGraph {
    // Adjacency: caller -> callees, seeded with every function name so that
    // naked functions (which may contain only `asm` and no calls) remain
    // nodes for the depth check. `Inst::Asm` is intentionally ignored here,
    // it carries no call edge, and falls through the `if let Call` below
    // (equivalent to `_ => continue`). No overlay layout change.
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    for f in &m.funcs {
        adj.entry(f.name.clone()).or_default();
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call(c) = inst {
                    edges.push((f.name.clone(), c.func.clone()));
                    adj.entry(f.name.clone()).or_default().push(c.func.clone());
                }
                // Inst::Asm and all other non-call instructions: no edge.
            }
        }
    }

    // DFS over the call graph: a back edge (revisit while in-progress) is a
    // cycle; otherwise record the longest call-chain depth ending at each node.
    let mut color: HashMap<String, u8> = HashMap::new(); // 0=white 1=gray 2=black
    let mut memo: HashMap<String, usize> = HashMap::new();
    let mut max_depth = 1usize;
    for f in &m.funcs {
        max_depth = max_depth.max(dfs_depth(&f.name, &adj, &mut color, &mut memo));
    }

    CallGraph { edges, max_depth }
}

fn dfs_depth(
    node: &str,
    adj: &HashMap<String, Vec<String>>,
    color: &mut HashMap<String, u8>,
    memo: &mut HashMap<String, usize>,
) -> usize {
    match color.get(node).copied().unwrap_or(0) {
        2 => return memo[node], // fully explored: reuse computed depth
        1 => panic!("callgraph: recursion detected (call cycle involving {node})"),
        _ => {}
    }
    color.insert(node.to_string(), 1);
    let mut deepest = 1usize;
    for callee in adj.get(node).map(Vec::as_slice).unwrap_or(&[]) {
        deepest = deepest.max(1 + dfs_depth(callee, adj, color, memo));
    }
    color.insert(node.to_string(), 2);
    memo.insert(node.to_string(), deepest);
    deepest
}

/// Render the call graph as a parseable edge list: one `edge <caller> <callee>`
/// line per edge, then `depth <max_depth>`. Consumed by the alloc stage.
pub fn edges_text(g: &CallGraph) -> String {
    let mut out = String::new();
    for (from, to) in &g.edges {
        out.push_str(&format!("edge {from} {to}\n"));
    }
    out.push_str(&format!("depth {}\n", g.max_depth));
    out
}

pub fn check_depth(g: &CallGraph, limit: usize) {
    assert!(g.max_depth <= limit, "callgraph: depth {} exceeds hardware stack {limit}", g.max_depth);
}
