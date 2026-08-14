/// Peephole-optimize PIC-8 assembly.
///
/// Milestone 1: pass-through. Later milestones add pattern-based cleanup
/// (e.g. dead `MOVWF`/`MOVF` pairs, redundant `CLRF`).
pub fn optimize(asm: &str) -> String {
    asm.to_string()
}
