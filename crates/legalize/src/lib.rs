//! Type-width validation boundary for the PIC8 pipeline.

use ir::{Module, parse, serialize};

pub fn legalize(m: Module) -> Module {
    // The `ir` parser already rejects non-i1/i8/i16 types, so v1 is a
    // pass-through boundary that later milestones extend (i16->i8 lowering,
    // runtime calls for mul/div).
    m
}
