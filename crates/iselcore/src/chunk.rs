//! Shared asm-text readers for cross-page chunk placement, used by the
//! PIC14 (`isel`) and PIC14E (`isel-pic14e`) backends.
//!
//! Only the leaf readers live here. The placement policies stay per
//! backend: classic reserves link plus banking slack behind a repair
//! loop, PIC14E plans exact three-word links, so the planners,
//! link inserters, restore rewrites, and unit placers cannot merge.

use std::collections::HashSet;

use ir::Func;

use crate::is_directive;

/// The code part of an asm line: comment and surrounding space removed.
pub fn code_of(line: &str) -> &str {
    line.split(';').next().unwrap_or("").trim()
}

/// The name a label line defines, or `None` for any other line. Dot
/// directives never count as labels: both backends exclude them at
/// every label test, so the rule lives here once.
pub fn label_of(code: &str) -> Option<&str> {
    let name = code.strip_suffix(':')?;
    (!name.contains(' ') && !name.starts_with('.')).then_some(name)
}

/// Whether a banked line occupies a flash word, the `word_size` rule one
/// line at a time so the chunk walk counts exactly what placement counts.
pub fn line_is_word(raw: &str) -> bool {
    let line = code_of(raw);
    if line.is_empty() {
        return false;
    }
    if is_directive(line, "list") || is_directive(line, "radix") {
        return false;
    }
    if line.starts_with("org ") {
        return false;
    }
    if is_directive(line, "end") {
        return false;
    }
    if line.ends_with(':') {
        return false;
    }
    if line.contains(" equ ") {
        return false;
    }
    if line.starts_with(".align ") {
        return false;
    }
    if line.starts_with(".table ") {
        return false;
    }
    true
}

/// The word size of a function's emitted lines: 1 word per instruction
/// line, mirroring the asm crate's pass-1 counting so the page-fit
/// decisions match the addresses the assembler will assign.
pub fn word_size(lines: &[String]) -> usize {
    lines.iter().filter(|raw| line_is_word(raw)).count()
}

/// Asm label of a function's block, the scheme `emit_func_body` uses: the
/// entry block is the bare function name, every other block `{func}_L{label}`.
pub fn block_asm_label(func: &str, index: usize, label: &str) -> String {
    if index == 0 {
        func.to_string()
    } else {
        format!("{func}_L{label}")
    }
}

/// Asm labels of a function's IR blocks in block order.
pub fn block_asm_labels(f: &Func) -> Vec<String> {
    f.blocks
        .iter()
        .enumerate()
        .map(|(i, b)| block_asm_label(&f.name, i, &b.label))
        .collect()
}

/// The banked lines of one function: from its label up to the next
/// function label or `__start`. Function labels are unique, so the slice
/// is exactly the lines banking measured for placement.
pub fn slice_banked_lines(banked: &str, func: &str, func_names: &HashSet<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for raw in banked.lines() {
        if let Some(label) = label_of(code_of(raw)) {
            if label == func {
                inside = true;
            } else if inside && (func_names.contains(label) || label == "__start") {
                break;
            }
        }
        if inside {
            out.push(raw.to_string());
        }
    }
    out
}
