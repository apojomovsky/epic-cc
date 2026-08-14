//! Single-module validation pass-through for the PIC8 pipeline.

use ir::{Module, parse, serialize};

pub fn merge(m: Module) -> Module {
    assert!(!m.funcs.is_empty(), "wholeprog: no functions in module");
    m
}
