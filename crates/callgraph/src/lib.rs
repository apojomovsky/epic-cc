//! Call graph and stack-depth check boundary for the PIC8 pipeline.

use ir::Module;

pub struct CallGraph {
    pub edges: Vec<(String, String)>,
    pub max_depth: usize,
}

pub fn build(_m: &Module) -> CallGraph {
    // Milestone 1: no call instructions exist in the straight-line subset, so the
    // graph is a forest of depth 1. The call milestone adds edges from call sites.
    CallGraph { edges: Vec::new(), max_depth: 1 }
}

pub fn check_depth(g: &CallGraph, limit: usize) {
    assert!(g.max_depth <= limit, "callgraph: depth {} exceeds hardware stack {limit}", g.max_depth);
}
