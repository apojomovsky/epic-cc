//! Cross-page placement for PIC14E functions past one 2048-word page
//! (epic-cc#880). A split function is cut at its block labels; each
//! non-final chunk ends in a three-word link to the next chunk's entry.
//! The classic `isel` crate holds the same helpers; they move to
//! `iselcore` once both backends share them.

use super::word_size;
use banking::SKIP_OPS;
use device::Device;
use ir::SrcLoc;
use std::collections::{HashMap, HashSet};

/// Words a non-final chunk spends on its link: `MOVLW PAGE(next)`,
/// `MOVWF PCLATH`, `GOTO next`. Reserved when planning, since no repair
/// pass follows placement to absorb growth.
pub(crate) const CHUNK_LINK_WORDS: usize = 3;

/// The code part of an asm line: comment and surrounding space removed.
fn code_of(line: &str) -> &str {
    line.split(';').next().unwrap_or("").trim()
}

/// The name a label line defines, or `None` for any other line.
fn label_of(code: &str) -> Option<&str> {
    let name = code.strip_suffix(':')?;
    (!name.contains(' ')).then_some(name)
}

/// Asm labels of a function's IR blocks in block order. The first is the
/// function label; the rest are `{name}_L{label}`, as `emit_func_body`
/// writes them.
pub(crate) fn block_asm_labels(f: &ir::Func) -> Vec<String> {
    f.blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if i == 0 {
                f.name.clone()
            } else {
                format!("{}_L{}", f.name, b.label)
            }
        })
        .collect()
}

/// Post-banking extent of each function and `__start`, label to label with
/// assembler semantics. Block labels stay inside the enclosing function,
/// matching the page-fit check.
pub(crate) fn post_extents(banked: &str, func_names: &HashSet<&str>) -> HashMap<String, usize> {
    let mut post: HashMap<String, usize> = HashMap::new();
    let mut org = 0usize;
    let mut cur: Option<(String, usize)> = None;
    for raw in banked.lines() {
        let line = code_of(raw);
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        if let Some(l) = line.strip_suffix(':') {
            let name = l.trim().to_string();
            if func_names.contains(name.as_str()) || name == "__start" {
                if let Some((prev, s)) = &cur {
                    post.insert(prev.clone(), org - s);
                }
                cur = Some((name, org));
            }
            continue;
        }
        if line.contains(" equ ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        if line.starts_with(".table ") {
            continue;
        }
        org += 1;
    }
    if let Some((prev, s)) = &cur {
        post.insert(prev.clone(), org - s);
    }
    post
}

/// The banked lines of one function: from its label up to the next
/// function label or `__start`. Banking has already run on these lines,
/// so BANKSEL growth is counted in the plan.
pub(crate) fn slice_banked_lines(
    banked: &str,
    func: &str,
    func_names: &HashSet<&str>,
) -> Vec<String> {
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

/// The target of a `GOTO` to a label defined in the same lines.
fn goto_target<'a>(code: &'a str, defined: &HashSet<&str>) -> Option<&'a str> {
    let target = code.strip_prefix("GOTO ")?;
    (!target.contains(' ') && defined.contains(target)).then_some(target)
}

/// Whether a word line is a skip op, which skips the word after it.
fn is_skip(line: &str) -> bool {
    let op = code_of(line).split_whitespace().next().unwrap_or("");
    SKIP_OPS.contains(&op)
}

/// Sets PCLATH before each `GOTO` to a label defined in `lines`, so the
/// jump reaches the target's page wherever the chunk landed. A set pair
/// goes before a skip op that directly precedes the GOTO, so the skip
/// still guards only the GOTO.
pub(crate) fn insert_goto_sets(
    lines: &[String],
    locs: &[Option<SrcLoc>],
) -> (Vec<String>, Vec<Option<SrcLoc>>) {
    let defined: HashSet<&str> = lines
        .iter()
        .filter_map(|l| label_of(code_of(l)))
        .filter(|name| !name.starts_with('.'))
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut out_locs: Vec<Option<SrcLoc>> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let loc = locs.get(i).cloned().flatten();
        if let Some(target) = goto_target(code_of(line), &defined) {
            let at = match out
                .iter()
                .rposition(|l| word_size(std::slice::from_ref(l)) > 0)
            {
                Some(j) if is_skip(&out[j]) => j,
                _ => out.len(),
            };
            let pair = [
                format!("    MOVLW PAGE({target})"),
                "    MOVWF PCLATH".to_string(),
            ];
            for (n, p) in pair.into_iter().enumerate() {
                out.insert(at + n, p);
                out_locs.insert(at + n, None);
            }
        }
        out.push(line.clone());
        out_locs.push(loc);
    }
    (out, out_locs)
}

/// Retargets each restore pair in `lines` from the function's page to the
/// page of the chunk it sits in. `entries` are the chunk entry labels. The
/// word count is unchanged, so planned sizes still hold.
pub(crate) fn rewrite_restores(
    func: &str,
    entries: &[String],
    mut lines: Vec<String>,
) -> Vec<String> {
    let restore = format!("    MOVLW PAGE({func})");
    let mut entry = func.to_string();
    for i in 0..lines.len() {
        let code = code_of(&lines[i]).to_string();
        if let Some(label) = label_of(&code) {
            if entries.iter().any(|e| e == label) {
                entry = label.to_string();
            }
            continue;
        }
        let is_restore = code == restore.trim()
            && lines
                .get(i + 1)
                .is_some_and(|next| code_of(next) == "MOVWF PCLATH");
        if is_restore {
            lines[i] = format!("    MOVLW PAGE({entry})");
        }
    }
    lines
}

/// Chunk plan for one function as `(entry label, words)`, first entry the
/// function label. Words cover the chunk's blocks and exclude the link;
/// each non-final chunk also holds `CHUNK_LINK_WORDS`. Panics when a block
/// cannot share a page with its link, or a boundary follows a skip op: the
/// link would then sit between the skip and the word it skips.
pub(crate) fn plan_chunks(
    func: &str,
    block_labels: &[String],
    lines: &[String],
) -> Vec<(String, usize)> {
    let blocks: HashSet<&str> = block_labels.iter().map(String::as_str).collect();
    // (entry label, words, follows a skip op) per block in emitted order.
    let mut seq: Vec<(String, usize, bool)> = Vec::new();
    let mut after_skip = false;
    for line in lines {
        let code = code_of(line);
        if let Some(label) = label_of(code) {
            if blocks.contains(label) {
                seq.push((label.to_string(), 0, after_skip));
            }
            continue;
        }
        let words = word_size(std::slice::from_ref(line));
        if words == 0 {
            continue;
        }
        let Some(block) = seq.last_mut() else {
            panic!("isel: function @{func} emits words before its first block label");
        };
        block.1 += words;
        after_skip = is_skip(code);
    }
    let Some((first, _, _)) = seq.first() else {
        panic!("isel: function @{func} has no block labels to split at");
    };
    let mut plan: Vec<(String, usize)> = Vec::new();
    let mut entry = first.clone();
    let mut acc = 0usize;
    for (label, size, skip_before) in &seq {
        if size + CHUNK_LINK_WORDS > 0x800 {
            panic!(
                "isel: function @{func} block {label} of {size} words exceeds a 2048-word page (0x800): a single block cannot span pages"
            );
        }
        if acc + size + CHUNK_LINK_WORDS > 0x800 {
            if *skip_before {
                panic!(
                    "isel: function @{func} cannot split before block {label}: it follows a skip op, and a chunk link would sit between the skip and the word it skips"
                );
            }
            plan.push((std::mem::replace(&mut entry, label.clone()), acc));
            acc = 0;
        }
        acc += size;
    }
    plan.push((entry, acc));
    plan
}

/// Cuts a function's lines at the plan entries for chunk `k`. Chunk `k`
/// runs from its entry to the next entry, then links to it. Chunk 0 starts
/// at the function label, which is the first line of `lines`.
pub(crate) fn chunk_slice(
    lines: &[String],
    locs: &[Option<SrcLoc>],
    plan: &[(String, usize)],
    k: usize,
) -> (Vec<String>, Vec<Option<SrcLoc>>) {
    let entry_index = |label: &str| -> usize {
        lines
            .iter()
            .position(|l| label_of(code_of(l)) == Some(label))
            .unwrap_or_else(|| panic!("isel: chunk entry {label} missing from the emitted text"))
    };
    let start = if k == 0 { 0 } else { entry_index(&plan[k].0) };
    let end = match plan.get(k + 1) {
        Some((next, _)) => entry_index(next),
        None => lines.len(),
    };
    let mut out: Vec<String> = lines[start..end].to_vec();
    let mut out_locs: Vec<Option<SrcLoc>> = (start..end)
        .map(|i| locs.get(i).cloned().flatten())
        .collect();
    if let Some((next, _)) = plan.get(k + 1) {
        out.push(format!("    MOVLW PAGE({next})"));
        out.push("    MOVWF PCLATH".to_string());
        out.push(format!("    GOTO {next}"));
        out_locs.extend([None, None, None]);
    }
    (out, out_locs)
}

/// Places a unit of `size` words first-fit into the lowest page tail with
/// room (`page_next[p]` is page `p`'s next free address), else opens the
/// next page. Returns `(page, start)`. Panics past the device flash.
pub(crate) fn place_unit(
    device: &Device,
    name: &str,
    size: usize,
    page_next: &mut Vec<usize>,
) -> (usize, usize) {
    for (pi, next) in page_next.iter_mut().enumerate() {
        if *next + size <= (pi + 1) * 0x800 {
            let start = *next;
            *next += size;
            return (pi, start);
        }
    }
    // Ceiling division: a device narrower than one page still has one.
    let pi = page_next.len();
    let num_pages = device.flash_words.div_ceil(0x800);
    let last_page = num_pages - 1;
    if pi as u32 >= num_pages {
        panic!(
            "isel: function @{name} would start at 0x{:04X}, beyond page {last_page} (device flash is {:#06x} words)",
            pi * 0x800,
            device.flash_words
        );
    }
    let start = pi * 0x800;
    page_next.push(start + size);
    (pi, start)
}
