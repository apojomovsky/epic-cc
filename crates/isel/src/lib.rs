//! `isel`: instruction selection for the integer spine.
//!
//! Scalar surface (milestones 2-6): lowers `load`/`store`, the full binop
//! set `add`/`sub`/`and`/`or`/`xor` (i8 and i16), `zext`/`sext`/`trunc`,
//! all ten `icmp` predicates (eq/ne/ult/ule/ugt/uge/slt/sle/sgt/sge),
//! `call` (arg copies into the callee's param slots + retval copy), `ret`
//! with and without a value, and eliminates `phi` by copying each
//! predecessor's incoming value into the phi destination at the end of the
//! predecessor block, before its terminator. Any other instruction or binop
//! panics.
//!
//! Phase-3 pointers/const (M5-M7): `gep` defines a *virtual* pointer
//! (emits nothing), lowered at each `load`/`store`/`memcpy` use. Every GEP
//! chain is resolved eagerly to a `(Base, k, terms)` triple: `Base::Global`
//! (RAM or const-flash), `Base::Slot(name, indirect)`: a byval param copy,
//! an alloca, or an sret slot holding a target address (indirect). A
//! constant offset (no terms) reads/writes the plain file register; dynamic
//! terms set `FSR` to `base + k + Σ s×%r` (single scale-1 term keeps the M5
//! `MOVF %r,W; ADDLW base+k; MOVWF FSR` fast path; general sums accumulate
//! in the fixed scratch byte); an indirect base takes the target address
//! from the slot's contents. Pointers into const (flash) globals load via
//! `CALL __read_<name>`: a RETLW table emitted after the functions, and a
//! store through a const base panics (ROM is not writable). `memcpy`
//! lowers to a byte loop of the same pointer machinery; `alloca` is virtual
//! like `gep` (the slot is sized by alloc). Static FSR bases reach all four
//! GPR banks via the IRP bit (M9): every FSR setup emits `BCF/BSF STATUS, 7`
//! (IRP = base bit 8) first, then loads FSR with `(base + k + off) & 0xFF`.
//! The FSR-accessed object must fit entirely inside one of the four GPR
//! windows `[0x20,0x80)` `[0xA0,0xF0)` `[0x120,0x170)` `[0x1A0,0x1F0)`:
//! crossing an SFR hole would silently mis-address, so it panics loudly at
//! emission (the object span comes from the global size / param width /
//! alloca size). An *indirect* (sret) base sets IRP from the stored
//! address's high byte (`BTFSC/BTFSS <slot+1>,0; BSF/BCF STATUS,7`) before
//! computing `FSR = [slot] + k + off`, so sret targets may sit in any bank
//! (the caller's sret store checks the target's window the same way).
//!
//! Every value's address comes from the caller-supplied address map: globals
//! by name, locals by `{func}::{name}` (IR value names without `%`). isel
//! performs no slot allocation; it trusts the map (from `alloc`'s overlay
//! layout) and panics loudly if a value is missing from it.

use device::Device;
use ir::{BinOp, Inst, MemLen, Module, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};
use std::collections::{HashMap, HashSet};

/// The recipe a routine function emits, or `None` if the name is not a
/// runtime routine at all. The name set itself is `ir::is_runtime_routine`,
/// shared with `alloc` (bank-straddle rounding needs the same list): an
/// injected routine's entry block holds only a scratch alloca, so emitting
/// it as-is would produce an empty label that silently falls through into
/// the next function. A routine name with no recipe yet must panic loudly
/// instead.
///
/// An interrupt-context copy (`__mul_u8_isr`, legalize's routine
/// duplication) shares the base routine's recipe but keeps its own name for
/// its label and, the load-bearing part, its own slots, so the ISR's frame
/// never overlaps the copy main is executing in. A duplicated USER function
/// (`helper_isr`) strips to `helper`, which is not a routine, so it takes
/// the ordinary block-emission path.
fn routine_recipe(name: &str) -> Option<&str> {
    let base = name.strip_suffix("_isr").unwrap_or(name);
    ir::is_runtime_routine(name).then_some(base)
}

/// The byte address of a literal-pointer operand (`"0x<K>"`, the
/// `inttoptr (<ty> <k> to ptr)` constant-pointer form parsed by irparse).
/// Used for direct (SFR) load/store: the register is bank-mirrored
/// (0x00-0x1F) or common RAM (0x70-0x7F), so no FSR setup and no BANKSEL.
fn literal_ptr_addr(ptr: &str) -> u16 {
    let lit = ptr
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("isel: malformed literal pointer {ptr:?}"));
    u16::from_str_radix(lit, 16)
        .unwrap_or_else(|_| panic!("isel: malformed literal pointer {ptr:?}"))
}

/// The GPR windows reachable through FSR+IRP, derived from the device's
/// `ram_banks` and `common_ram`. Each bank's window is the GPR bank itself
/// plus, for the first bank, the common RAM that is bank-mirrored. The
/// concrete windows for the 877A are `[0x20,0x80) [0xA0,0xF0) [0x120,0x170)
/// [0x1A0,0x1F0)`; other PIC14 parts derive analogously from their
/// `ram_banks`/`common_ram`.
fn fsr_window(device: &Device, base_addr: u16, span: u16) -> (bool, u8) {
    // Derive windows from the device. First bank's window extends through
    // common RAM when common directly follows its GPR (0x6F -> 0x70-0x7F -> 0x80).
    let mut windows: Vec<(u16, u16)> = Vec::new();
    for (i, (lo, hi)) in device.ram_banks.iter().enumerate() {
        let mut end = hi + 1;
        if i == 0 {
            if let Some((clo, chi)) = device.common_ram {
                if clo == hi + 1 {
                    end = chi + 1;
                }
            }
        }
        windows.push((*lo, end));
    }
    // Common RAM addresses are inside the first window; treat them as bank 0.
    let win_end = if let Some((clo, chi)) = device.common_ram {
        if base_addr >= clo && base_addr <= chi {
            windows[0].1
        } else {
            let mut found = None;
            for ((lo, hi), (_, we)) in device.ram_banks.iter().zip(&windows) {
                if base_addr >= *lo && base_addr <= *hi {
                    found = Some(*we);
                    break;
                }
            }
            found.unwrap_or_else(|| {
                panic!(
                    "isel: FSR base 0x{base_addr:03X} outside GPR space (device {})",
                    device.name
                )
            })
        }
    } else {
        let mut found = None;
        for ((lo, hi), (_, we)) in device.ram_banks.iter().zip(&windows) {
            if base_addr >= *lo && base_addr <= *hi {
                found = Some(*we);
                break;
            }
        }
        found.unwrap_or_else(|| {
            panic!(
                "isel: FSR base 0x{base_addr:03X} outside GPR space (device {})",
                device.name
            )
        })
    };
    assert!(
        base_addr + span <= win_end,
        "isel: FSR object at 0x{base_addr:03X} span {span} crosses window end 0x{win_end:X} (SFR hole)"
    );
    (((base_addr >> 8) & 1) == 1, (base_addr & 0xFF) as u8)
}

/// How a single-byte pointer access completes after `emit_ptr_setup`.
enum Addr {
    /// A plain file register (the address is statically known).
    Direct(u16),
    /// FSR is set up; the access goes through INDF.
    Indirect,
}

/// Per-function codegen state. All addresses come from the module-wide map;
/// `cur_func` selects the current function's local entries.
struct Gen<'m> {
    m: &'m Module,
    addrs: &'m HashMap<String, u16>,
    device: &'m Device,
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)`: GEP chains fully collapsed (base
    /// `Reg` replaced by the base's own entry), plus the seeded pointer
    /// bases (byval/sret params and allocas). `gep`/`alloca` themselves
    /// emit nothing; each `load`/`store`/`memcpy` through a pointer reg
    /// lowers the pointer at its use.
    resolved: &'m HashMap<String, (Base, u8, Vec<(u8, String)>)>,
    scratch: u16,
    retval_lo: u16,
    cur_func: &'m str,
    /// Module-scoped fresh-label counter, shared across every function so the
    /// emitted `tmp{n}:` labels stay unique in the single `.asm` output.
    tmp: &'m mut u32,
    /// Page map (M11, two-phase emission): every CALL target and const-reader
    /// entry -> the page PCLATH<4:3> holds AFTER the CALL returns (a
    /// function callee never clobbers it; a reader leaves `HIGH(<base>)`,
    /// whose bits 4:3 are the table base's page). `None` in pass A, where
    /// pages are not yet known and every restore is emitted (the measured
    /// sizes drive the page assignment); `Some` in pass B, where a
    /// same-page restore is skipped.
    page_of: Option<&'m HashMap<String, usize>>,
    out: Vec<String>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    /// The M11 restore pair: `MOVLW PAGE(<cur_func>); MOVWF PCLATH`, right
    /// after a CALL. Skipped when the called target runs in the current
    /// function's own page: the set already wrote that page and nothing since
    /// changed PCLATH<4:3> (a function callee restores itself; a const
    /// reader leaves `HIGH(<base>)` whose bits 4:3 are the table base's page,
    /// equal to the target's page). In pass A (`page_of` is `None`) the
    /// pages are not known yet, so the restore is always emitted: pass A's
    /// sizes (with every restore) drive the page assignment, and the
    /// pass-B skip only shrinks functions, which never moves a function off
    /// its assigned page (the `.org` pads pin the page bases).
    fn emit_pclath_restore(&mut self, target: &str) {
        let same_page = match self.page_of {
            Some(pages) => pages.get(target) == pages.get(self.cur_func),
            None => false,
        };
        if !same_page {
            self.emit(format!("    MOVLW PAGE({})", self.cur_func));
            self.emit("    MOVWF PCLATH".to_string());
        }
    }

    /// Resolve `{func}::{name}` to its base byte address (lo for multi-byte).
    /// Every address comes from the caller-supplied map; a missing value
    /// panics loudly rather than being allocated internally.
    /// True when `name` is a plain pointer param of the current function, whose
    /// slot holds a runtime address rather than being the object itself.
    fn param_holds_addr(&self, name: &str) -> bool {
        self.m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .map(|f| f.params.iter().any(|p| p.name == name && p.ptr))
            .unwrap_or(false)
    }

    fn slot_addr(&self, func: &str, name: &str) -> Slot {
        Slot::Direct(
            *self
                .addrs
                .get(&ssa_key(func, name))
                .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}")),
        )
    }

    /// Substitute `$0`/`%0` placeholders in an inline asm template with the
    /// allocated address of each `*m` operand. Each operand's `ptr` is either
    /// `@global` or `%local`; globals are looked up directly, locals via
    /// `ssa_key(func, name)`. GEP-derived pointers panic per D-3.
    fn substitute_asm(&self, template: &str, operands: &[ir::AsmOperand]) -> String {
        // Detect GEP-derived operand pointers: any `%reg` that resolves to a
        // GEP with non-zero offset or dynamic terms is not a direct local.
        for op in operands {
            if let Some(reg) = op.ptr.strip_prefix('%') {
                if let Some((_, k, terms)) = self.resolved.get(&ssa_key(self.cur_func, reg)) {
                    if *k != 0 || !terms.is_empty() {
                        panic!("asm: GEP-derived pointers are not supported; operand {} is derived via getelementptr (only direct locals and globals are allowed)", op.ptr);
                    }
                }
            }
        }
        let mut out = String::with_capacity(template.len() + operands.len() * 6);
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '$' || c == '%' {
                if let Some(&next) = chars.peek() {
                    if next == '%' || next == '$' {
                        // escaped `%%` or `$$` -> literal second char
                        chars.next();
                        out.push(next);
                        continue;
                    }
                    if next.is_ascii_digit() {
                        let mut idx_str = String::new();
                        while let Some(&d) = chars.peek() {
                            if d.is_ascii_digit() {
                                idx_str.push(d);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        let idx: usize = idx_str.parse().unwrap();
                        if idx >= operands.len() {
                            panic!("asm: placeholder ${idx} out of range for {} operands in template {template:?}", operands.len());
                        }
                        let ptr = &operands[idx].ptr;
                        let addr = if let Some(g) = ptr.strip_prefix('@') {
                            *self
                                .addrs
                                .get(g)
                                .unwrap_or_else(|| panic!("isel: no address for @{g}"))
                        } else if let Some(r) = ptr.strip_prefix('%') {
                            self.slot_addr(self.cur_func, r).direct()
                        } else {
                            panic!("asm: malformed operand ptr {ptr:?}");
                        };
                        out.push_str(&format!("0x{addr:02X}"));
                        continue;
                    }
                }
                out.push(c);
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> Slot {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
            Val::Global(g) => Slot::Direct(
                *self
                    .addrs
                    .get(g)
                    .unwrap_or_else(|| panic!("isel: no address for @{g}")),
            ),
            Val::Const(k) => {
                // Mask to the byte: clang prints i8 constants >= 128 as
                // negative i8 (found by the fuzz corpus); the value is the
                // same mod 256.
                Slot::Direct((*k & 0xFF) as u16)
            }
        }
    }

    /// The byte address of a RAM global (from the map).
    fn global_addr(&self, name: &str) -> u16 {
        *self
            .addrs
            .get(name)
            .unwrap_or_else(|| panic!("isel: no address for @{name}"))
    }

    /// Whether `name` is a const (flash) global: read via RETLW tables.
    fn global_is_const(&self, name: &str) -> bool {
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .is_const
    }

    /// The byte size of a global: a const table's size selects the reader
    /// shape (≤ 255 single entry; ≥ 256 two chunked entries, chunk 1 empty
    /// for exactly 256 bytes).
    fn global_size(&self, name: &str) -> u16 {
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .size
    }

    /// The byte width of an SSA value reg in the current function, from its
    /// defining param or instruction. Used to verify the large-table GEP
    /// index is the 16-bit reg clang zexts: reading the hi slot of a
    /// 1-byte index would silently touch a neighbour's slot.
    fn reg_bytes(&self, name: &str) -> u8 {
        let f = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == self.cur_func)
            .unwrap_or_else(|| panic!("isel: unknown function {}", self.cur_func));
        if let Some(p) = f.params.iter().find(|p| p.name == name) {
            return p.width;
        }
        for b in &f.blocks {
            for i in &b.insts {
                let w = match i {
                    Inst::Load(l) if l.dst == name => l.ty.bytes(),
                    Inst::Bin(b) if b.dst == name => b.ty.bytes(),
                    Inst::Zext(z) if z.dst == name => z.to.bytes(),
                    Inst::Sext(x) if x.dst == name => x.to.bytes(),
                    Inst::Trunc(t) if t.dst == name => t.to.bytes(),
                    Inst::Icmp(c) if c.dst == name => 1,
                    Inst::Select(s) if s.dst == name => s.ty.bytes(),
                    Inst::Phi(p) if p.dst == name => p.ty.bytes(),
                    Inst::Freeze(fr) if fr.dst == name => fr.ty.bytes(),
                    Inst::Call(c) if c.dst.as_deref() == Some(name) => {
                        c.ty.map(|t| t.bytes()).unwrap_or(1)
                    }
                    Inst::FloatBin(fb) if fb.dst == name => 4,
                    Inst::Fcmp(fc) if fc.dst == name => 1,
                    Inst::FloatConv(fc) if fc.dst == name => fc.to.bytes(),
                    _ => continue,
                };
                return w;
            }
        }
        panic!("isel: no definition of %{name} in {}", self.cur_func);
    }

    /// The byte span of a resolved FSR base: the whole object a pointer
    /// into it can legally touch (the runtime terms are bounded by span−1).
    /// `Base::Global` spans its `Global.size`; `Base::Slot` spans the byval
    /// param's `width` or the alloca's `size` in the current function. A
    /// missing object panics loudly.
    fn object_span(&self, base: &Base) -> u16 {
        match base {
            Base::Global(name) => {
                self.m
                    .globals
                    .iter()
                    .find(|g| g.name == *name)
                    .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
                    .size as u16
            }
            Base::Slot(sname, _) => {
                let f = self
                    .m
                    .funcs
                    .iter()
                    .find(|f| f.name == self.cur_func)
                    .unwrap_or_else(|| {
                        panic!(
                            "isel: no span for slot {sname}: unknown function {}",
                            self.cur_func
                        )
                    });
                if let Some(p) = f.params.iter().find(|p| p.name == *sname) {
                    p.width as u16
                } else if let Some(a) = f.blocks.iter().flat_map(|b| &b.insts).find_map(|i| {
                    if let Inst::Alloca(a) = i {
                        (a.dst == *sname).then_some(a)
                    } else {
                        None
                    }
                }) {
                    a.size as u16
                } else {
                    panic!("isel: no span for slot {sname} in {}", self.cur_func);
                }
            }
        }
    }

    /// The resolved `(base, k, terms)` for a pointer reg `%r`: a GEP dst,
    /// or a seeded byval/sret param / alloca. Anything else is a missing
    /// pointer and panics loudly.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
    }

    /// How a byte access at `ptr + byte_off` completes: `Direct(a)` reads or
    /// writes the plain file register `a`; `Indirect` means FSR is already
    /// set up and the access goes through INDF. Emits the address setup for
    /// dynamic/indirect pointers. Const (flash) bases are rejected (loads
    /// take the RETLW path before this; stores panic).
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => {
                assert!(
                    !self.global_is_const(g),
                    "isel: store to const (flash) global @{g}"
                );
                Addr::Direct(self.global_addr(g) + u16::from(byte_off))
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        assert!(
                            !self.global_is_const(name),
                            "isel: store to const (flash) global @{name}"
                        );
                        if terms.is_empty() {
                            // Constant offset only: the address is statically
                            // known, a plain file-register access, no FSR.
                            Addr::Direct(
                                self.global_addr(name) + u16::from(k) + u16::from(byte_off),
                            )
                        } else {
                            let span = self.object_span(&base);
                            self.emit_fsr_to(self.global_addr(name), k, &terms, byte_off, span);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        // A plain pointer param's slot holds the address rather
                        // than being the object, so it is read like an `sret`
                        // slot (a `byval` param's slot IS the object).
                        if *indirect || self.param_holds_addr(sname) {
                            self.emit_fsr_indirect(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else if terms.is_empty() {
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
                        } else {
                            let span = self.object_span(&base);
                            self.emit_fsr_to(sa, k, &terms, byte_off, span);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// `W = RAM[ptr + byte_off]`: one byte of a pointer load or a memcpy
    /// source. Direct bases read the plain file register; dynamic bases set
    /// FSR first and read INDF; a const (flash) base reads via
    /// `CALL __read_<name>` (the RETLW table leaves the byte in W). A table
    /// larger than 255 bytes takes the 16-bit index path: the caller
    /// splits the index into an in-chunk byte (W) and the chunk bit, then
    /// CALLs `__read_<name>` (chunk 0) or `__read_<name>_hi` (chunk 1).
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), k, terms) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        if self.global_size(&name) > 255 {
                            // Large table: W = in-chunk index, hi bit in
                            // 0x70, branch to the right chunk entry.
                            self.emit_const_read_large(&name, k, &terms, byte_off);
                        } else {
                            // RETLW table read: W = index = k + Σ s×%reg + off.
                            // The reader's input is W itself, so the set (whose
                            // MOVLW clobbers W) goes BEFORE the index
                            // computation; the index is computed into W after,
                            // and nothing between touches PCLATH. The restore
                            // right after the CALL saves the returned byte in
                            // the fixed scratch (free at a const read) across
                            // its own MOVLW, then reloads it into W.
                            self.emit(format!("    MOVLW PAGE(__read_{name})"));
                            self.emit("    MOVWF PCLATH".to_string());
                            self.emit_ptr_index_w(k, &terms, byte_off);
                            self.emit(format!("    CALL __read_{name}"));
                            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                            self.emit_pclath_restore(&format!("__read_{name}"));
                            self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                        }
                        return;
                    }
                }
            }
            Val::Global(g) => {
                if self.global_is_const(g) {
                    // A const global used directly as a pointer (memcpy src):
                    // W = byte index, CALL the RETLW table reader. The index
                    // is a compile-time constant, so a large table (whose
                    // chunk must be selected) panics loudly for now.
                    assert!(
                        self.global_size(g) <= 255,
                        "isel: constant index into large const table @{g} not supported (size {} > 255); only a single 16-bit reg index is",
                        self.global_size(g)
                    );
                    self.emit(format!("    MOVLW PAGE(__read_{g})"));
                    self.emit("    MOVWF PCLATH".to_string());
                    self.emit(format!("    MOVLW 0x{byte_off:02X}"));
                    self.emit(format!("    CALL __read_{g}"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit_pclath_restore(&format!("__read_{g}"));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                    return;
                }
            }
            Val::Const(_) => panic!("isel: load through a constant pointer"),
        }
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => self.emit(format!("    MOVF 0x{a:02X}, W")),
            Addr::Indirect => self.emit("    MOVF INDF, W".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = W`: the store side of a byte access (memcpy
    /// destinations; `emit_ptr_store_byte` composes a val load before it).
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => self.emit(format!("    MOVWF 0x{a:02X}")),
            Addr::Indirect => self.emit("    MOVWF INDF".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        // The address setup comes first: its FSR/scratch computation
        // clobbers W, so the value is loaded only after FSR is final.
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_load_byte(val, byte_off);
                self.emit(format!("    MOVWF 0x{a:02X}"));
            }
            Addr::Indirect => {
                self.emit_load_byte(val, byte_off);
                self.emit("    MOVWF INDF".to_string());
            }
        }
    }

    /// Runtime-length memcpy (issue #4): `len` is a 16-bit SSA register,
    /// SSA-dead after this copy, so the copy may consume it. The loop
    /// copies one byte per iteration; EVERY byte of the loop state lives
    /// in fixed common RAM: countdown 0x71/0x72 (the retval bytes, dead
    /// at a memcpy), byte index 0x7E (the documented free byte), held byte
    /// 0x7F, so the banking pass can never insert a BANKSEL inside the
    /// loop and the skip-sensitive test/branch pairs keep their targets:
    ///
    ///   count = len                          ; 0x71 = lo, 0x72 = hi
    ///   idx = 0                              ; 0x7E
    ///   l_loop:  if (0x71 | 0x72) == 0 -> l_done
    ///            FSR = src_base + k + terms + idx; W = INDF; 0x7F = W
    ///            FSR = dst_base + k + terms + idx; W = 0x7F; INDF = W
    ///            idx++                       ; INCF 0x7E
    ///            countdown-- (16-bit)        ; MOVLW 1 / SUBWF 0x71,F /
    ///                                         ; BTFSS STATUS,0 / SUBWF 0x72,F
    ///            GOTO l_loop
    ///   l_done:
    ///
    /// The 16-bit countdown decrements the lo byte with `MOVLW 1; SUBWF
    /// 0x71,F`: C = 0 exactly when lo wrapped (was 0), and the `BTFSS
    /// STATUS,0` skips the hi byte's decrement on no-borrow, so hi
    /// decrements once per lo wrap: exact for any length. The zero test at
    /// the top (`MOVF 0x71,W; IORWF 0x72,W; BTFSC STATUS,2`) skips an
    /// empty copy.
    /// The FSR setups read the pointer's term registers (any bank, those
    /// reads are not inside a skip pair), and the source/dest bases are
    /// window-checked per byte by `emit_fsr_to` (span <= 0x60), so a valid
    /// program's `idx` never leaves the window. Copying a const (flash)
    /// source at a runtime length panics loudly (the byte-at-a-time flash
    /// reader needs the index in W, which the loop's FSR discipline does
    /// not provide: the constant-length path covers flash sources).
    fn emit_memcpy_dynamic(&mut self, dst: &Val, src: &Val, len: &Val) {
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        let cnt_lo: u16 = self.retval_lo; // 0x71, dead at a memcpy
        let cnt_hi: u16 = self.retval_lo + 1; // 0x72
        let idx: u16 = 0x7E; // documented free common byte
        let hold: u16 = 0x7F; // documented free common byte
                              // FSR = base + k + terms + idx for one byte of a pointer; the FSR
                              // must be re-set per byte (one FSR on classic mid-range).
        let emit_byte_fsr = |g: &mut Self, ptr: &Val| {
            let (base, k, terms) = match ptr {
                Val::Reg(r) => g.resolved_for(r),
                Val::Global(gname) => {
                    assert!(
                        !g.global_is_const(gname),
                        "isel: memcpy into const (flash) global @{gname}"
                    );
                    (Base::Global(gname.clone()), 0u8, Vec::new())
                }
                _ => panic!("isel: dynamic memcpy ptr must be a reg or global"),
            };
            match &base {
                Base::Global(name) => {
                    assert!(
                        !g.global_is_const(name),
                        "isel: dynamic memcpy of a const (flash) source @{name} is not supported (runtime length; use a constant length for flash sources)"
                    );
                    let span = g.object_span(&base);
                    g.emit_fsr_to(g.global_addr(name), k, &terms, 0, span);
                }
                Base::Slot(sname, indirect) => {
                    assert!(!indirect, "isel: dynamic memcpy through an indirect slot");
                    let sa = g.slot_addr(g.cur_func, sname).direct();
                    let span = g.object_span(&base);
                    g.emit_fsr_to(sa, k, &terms, 0, span);
                }
            }
            g.emit(format!("    MOVF 0x{idx:02X}, W"));
            g.emit("    ADDWF FSR, F".to_string());
        };
        // Length source slot (the SSA reg's own bytes, read once).
        let la = self.val_addr(len).direct();
        // countdown = len.
        self.emit(format!("    MOVF 0x{la:02X}, W"));
        self.emit(format!("    MOVWF 0x{cnt_lo:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", la + 1));
        self.emit(format!("    MOVWF 0x{cnt_hi:02X}"));
        // idx = 0.
        self.emit(format!("    CLRF 0x{idx:02X}"));
        self.emit(format!("{l_loop}:"));
        // Zero test: (cnt_lo | cnt_hi) == 0 -> done. All common RAM, so
        // no BANKSEL can appear between the BTFSC and its GOTO.
        self.emit(format!("    MOVF 0x{cnt_lo:02X}, W"));
        self.emit(format!("    IORWF 0x{cnt_hi:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_done}"));
        // src[i] -> hold.
        emit_byte_fsr(self, src);
        self.emit("    MOVF INDF, W".to_string());
        self.emit(format!("    MOVWF 0x{hold:02X}"));
        // dst[i] = hold.
        emit_byte_fsr(self, dst);
        self.emit(format!("    MOVF 0x{hold:02X}, W"));
        self.emit("    MOVWF INDF".to_string());
        // idx++.
        self.emit(format!("    INCF 0x{idx:02X}, F"));
        // countdown-- (16-bit): `MOVLW 1; SUBWF lo,F` sets C = 1 when lo
        // was >= 1 (no borrow) and 0 when lo was 0 (borrow, lo wrapped to
        // 0xFF). BTFSS skips the hi-byte decrement exactly when there is
        // no borrow, so the hi byte decrements once per wrap. (The encoder
        // has no plain DECF, only DECFSZ.) Both SUBWF targets are common
        // RAM, so no BANKSEL can land inside this skip pair.
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{cnt_lo:02X}, F"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    SUBWF 0x{cnt_hi:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_done}:"));
    }

    /// `FSR = base_addr + k + byte_off + Σ scale×%reg`, for an object of
    /// `span` bytes at `base_addr`. The window check runs first: the whole
    /// object must fit inside its base's GPR window (an unrepresentable
    /// cross-hole address would silently mis-address through INDF, so it
    /// panics loudly). IRP is then set on EVERY FSR setup: a prior
    /// bank-2/3 access leaves STATUS bit 7 = 1, so skipping the set on a
    /// bank-0/1 base would mis-address into bank 2/3. A single scale-1
    /// term keeps the M5 fast shape (`MOVF %r,W; ADDLW lit; MOVWF FSR`);
    /// general sums accumulate in the fixed scratch byte first. The ADDLW
    /// literal is `(base_addr + k + byte_off) & 0xFF`: FSR holds the low
    /// byte; IRP carries bit 8.
    fn emit_fsr_to(
        &mut self,
        base_addr: u16,
        k: u8,
        terms: &[(u8, String)],
        byte_off: u8,
        span: u16,
    ) {
        let (irp, base_lo) = fsr_window(self.device, base_addr, span);
        let lit = (u16::from(base_lo) + u16::from(k) + u16::from(byte_off)) & 0xFF;
        self.emit(if irp {
            "    BSF STATUS, 7".to_string()
        } else {
            "    BCF STATUS, 7".to_string()
        });
        match terms {
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDLW 0x{lit:02X}"));
                self.emit("    MOVWF FSR".to_string());
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit(format!("    ADDLW 0x{lit:02X}"));
                self.emit("    MOVWF FSR".to_string());
            }
        }
    }

    /// Indirect (sret) FSR setup: `FSR = [slot] + k + byte_off + Σ terms`.
    /// The slot holds the target address (the caller stores LOW then HIGH
    /// of it into the two slot bytes), so IRP is set from the stored HIGH
    /// byte BEFORE the FSR computation: bit 0 of `<slot+1>` is the
    /// address's bit 8; 1 -> IRP=1 (banks 2/3), 0 -> IRP=0 (banks 0/1).
    /// Exactly one of the pair fires: BTFSC skips the BSF when the bit is
    /// 0, BTFSS skips the BCF when it is 1, so IRP always matches the
    /// stored address. IRP is set on EVERY indirect FSR setup (a prior
    /// bank-2/3 target leaves STATUS bit 7 = 1). The static k + off must
    /// fit the ADDLW literal.
    fn emit_fsr_indirect(&mut self, slot_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: indirect offset k {k} + off {byte_off} out of byte range"
        );
        let hi = slot_addr + 1;
        self.emit(format!("    BTFSC 0x{hi:02X}, 0"));
        self.emit("    BSF STATUS, 7".to_string());
        self.emit(format!("    BTFSS 0x{hi:02X}, 0"));
        self.emit("    BCF STATUS, 7".to_string());
        if terms.is_empty() {
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit(format!("    ADDLW 0x{kk:02X}"));
            self.emit("    MOVWF FSR".to_string());
        } else {
            self.emit_accum_terms(terms);
            self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
            self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
            self.emit(format!("    ADDLW 0x{kk:02X}"));
            self.emit("    MOVWF FSR".to_string());
        }
    }

    /// `scratch = Σ scale×%reg`: W = 0, then per term
    /// `MOVF %r,W; ADDWF scratch,W; MOVWF scratch` repeated `scale` times.
    /// ADDWF f,W computes W = f + W, so W holds %r only until the first
    /// ADDWF: it MUST be reloaded before each repetition or a scaled term
    /// accumulates 2×scratch + %r (silent wrong-address miscompile).
    fn emit_accum_terms(&mut self, terms: &[(u8, String)]) {
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        for (scale, r) in terms {
            let a = self.val_addr(&Val::Reg(r.clone())).direct();
            for _ in 0..*scale {
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            }
        }
    }

    /// `W = k + byte_off + Σ scale×%reg`: the byte index into a const
    /// (flash) table before `CALL __read_<name>`. A single scale-1 term
    /// keeps the M5 `MOVF %r,W` shape (ADDLW only when k + off is nonzero);
    /// general sums accumulate in scratch.
    fn emit_ptr_index_w(&mut self, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: const index k {k} + off {byte_off} out of byte range"
        );
        match terms {
            [] => self.emit(format!("    MOVLW 0x{kk:02X}")),
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                if kk != 0 {
                    self.emit(format!("    ADDLW 0x{kk:02X}"));
                }
            }
            _ => {
                self.emit_accum_terms(terms);
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit(format!("    ADDLW 0x{kk:02X}"));
            }
        }
    }

    /// Large const table (> 255 bytes) read. The 16-bit GEP index splits
    /// into the in-chunk index (W) and the chunk number (hi temp, fixed
    /// scratch 0x70). For up to two chunks the chunk number is a single bit
    /// tested with `BTFSC 0x70,0`, the exact M10/M13 sequences, kept
    /// byte-identical so the committed fixtures hold (a 256-byte table
    /// emits an empty, unreachable chunk 1 the same way). For 3+ chunks
    /// (issue #8) the hi temp is the full chunk number and a descending
    /// `scratch >= c` chain selects the reader: `MOVLW 0x100-c; ADDWF
    /// scratch,W` sets C iff scratch >= c, so testing c = n-1 down to 1 in
    /// that order branches to the matching entry and falls through to
    /// chunk 0. W is the in-chunk index; the reader CALL's PCLATH set
    /// clobbers W, reloaded from the lo temp (0x71, retval_lo, no live
    /// retval at a const read); the returned byte is parked in the hi temp
    /// (0x70, dead after the chunk tests) across the restore. The reads
    /// leave the byte in W, exactly like the small-table path. Const-only
    /// and multi-term 16-bit indices panic loudly.
    fn emit_const_read_large(&mut self, name: &str, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: const index k {k} + off {byte_off} out of byte range"
        );
        let size = self.global_size(name) as usize;
        let chunks = (size + 255) / 256; // 256-byte tables: 1 (empty chunk 1)
        let disp = chunks.max(2); // dispatch shape: bit-0 test or >= c chain
                                  // The reader entry for chunk c: `__read_<name>`, `__read_<name>_hi`,
                                  // `__read_<name>_hi{c}`, matching the table emitter.
        let entry = |c: usize| {
            if c == 0 {
                format!("__read_{name}")
            } else if c == 1 {
                format!("__read_{name}_hi")
            } else {
                format!("__read_{name}_hi{c}")
            }
        };
        // `CALL __read_...; MOVWF <hi temp>; restore; MOVF <hi temp>,W;
        // GOTO l_done`: W holds the in-chunk index (the PCLATH set's
        // MOVLW clobbers it, reloaded from the lo temp); the returned byte
        // survives the restore via the hi temp.
        let chunk_call = |g: &mut Self, c: usize, l_done: &str| {
            let e = entry(c);
            g.emit(format!("    MOVLW PAGE({e})"));
            g.emit("    MOVWF PCLATH".to_string());
            g.emit(format!("    MOVF 0x{:02X}, W", g.retval_lo));
            g.emit(format!("    CALL {e}"));
            g.emit(format!("    MOVWF 0x{:02X}", g.scratch));
            g.emit_pclath_restore(&e);
            g.emit(format!("    MOVF 0x{:02X}, W", g.scratch));
            g.emit(format!("    GOTO {l_done}"));
        };
        // The 3+ chunk dispatch chain: descending `scratch >= c` tests
        // (`MOVLW 0x100-c; ADDWF scratch,W` sets C iff scratch >= c). Each
        // test branches to the c-th chunk's call; the fall-through after the
        // lowest test is chunk 0's call, and every call lands on `l_done`.
        let emit_chain = |g: &mut Self, l_done: &str| {
            let mut l_calls: Vec<(String, usize)> = Vec::new();
            for c in (1..disp).rev() {
                let l = g.fresh_label();
                l_calls.push((l.clone(), c));
                g.emit(format!("    MOVLW 0x{:02X}", 0x100 - c as u16));
                g.emit(format!("    ADDWF 0x{:02X}, W", g.scratch));
                g.emit("    BTFSC STATUS, 0".to_string());
                g.emit(format!("    GOTO {l}"));
            }
            chunk_call(g, 0, l_done);
            for (l, c) in l_calls {
                g.emit(format!("{l}:"));
                chunk_call(g, c, l_done);
            }
        };
        match terms {
            [(1, r)] => {
                assert_eq!(
                    self.reg_bytes(r),
                    2,
                    "isel: large-table index %{r} must be a 16-bit reg (clang zexts the byte index)"
                );
                let a_lo = self.val_addr(&Val::Reg(r.clone())).direct();
                let l_done = self.fresh_label();
                // W = lo + k + off; C = carry into bit 8.
                self.emit(format!("    MOVF 0x{a_lo:02X}, W"));
                self.emit(format!("    ADDLW 0x{kk:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo)); // lo temp
                // W = hi + carry.
                self.emit(format!("    MOVF 0x{:02X}, W", a_lo + 1));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch)); // hi temp (chunk)
                if disp == 2 {
                    // M10 exact: bit 0 of the hi temp selects the entry.
                    let l_hi = self.fresh_label();
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    BTFSC 0x{:02X}, 0", self.scratch));
                    self.emit(format!("    GOTO {l_hi}"));
                    self.emit(format!("    MOVLW PAGE(__read_{name})"));
                    self.emit("    MOVWF PCLATH".to_string());
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    CALL __read_{name}"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit_pclath_restore(&format!("__read_{name}"));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                    self.emit(format!("    GOTO {l_done}"));
                    self.emit(format!("{l_hi}:"));
                    self.emit(format!("    MOVLW PAGE(__read_{name}_hi)"));
                    self.emit("    MOVWF PCLATH".to_string());
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    CALL __read_{name}_hi"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit_pclath_restore(&format!("__read_{name}_hi"));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                } else {
                    emit_chain(self, &l_done);
                }
                self.emit(format!("{l_done}:"));
            }
            [(scale, r)] => {
                // Multi-byte elements (i16/f32 scale 2, i32/f32 scale 4,
                // classic mid-range has no MULLW): byte index = s×idx + k +
                // off. For a 2-chunk table (idx_hi == 0 in bounds) the lo
                // byte is shifted and the carry folded, the exact M13
                // sequence. For 3+ chunks the hi byte participates: the
                // shift pair accumulated `s*idx_lo >> 8` into the hi temp,
                // then `s*idx_hi` is added (in bounds
                // s*idx_hi + (s*idx_lo >> 8) + carry <= 255), so the hi
                // temp is then the exact chunk number.
                assert_eq!(
                    self.reg_bytes(r),
                    2,
                    "isel: large-table index %{r} must be a 16-bit reg"
                );
                assert!(
                    *scale == 2 || *scale == 4,
                    "isel: large-table element scale {scale} not supported (i16/i32/float only)"
                );
                let a_lo = self.val_addr(&Val::Reg(r.clone())).direct();
                let l_done = self.fresh_label();
                let pairs = match *scale {
                    2 => 1,
                    4 => 2,
                    _ => unreachable!(),
                };
                self.emit(format!("    MOVF 0x{a_lo:02X}, W"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo)); // lo temp
                self.emit(format!("    CLRF 0x{:02X}", self.scratch)); // hi temp
                self.emit("    BCF STATUS, 0".to_string());
                for _ in 0..pairs {
                    self.emit(format!("    RLF 0x{:02X}, F", self.retval_lo));
                    self.emit(format!("    RLF 0x{:02X}, F", self.scratch));
                }
                if chunks >= 3 {
                    // scratch += s*idx_hi (scale ADDWF of idx_hi, one per
                    // element byte of the scale, e.g. 2 adds for i16).
                    self.emit(format!("    MOVF 0x{:02X}, W", a_lo + 1));
                    for _ in 0..*scale {
                        self.emit(format!("    ADDWF 0x{:02X}, F", self.scratch));
                    }
                }
                // W = lo + kk; C = carry into bit 8.
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                self.emit(format!("    ADDLW 0x{kk:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                // hi += carry; the hi temp is the chunk.
                self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                if disp == 2 {
                    // M13 exact: bit 0 of the hi temp selects the entry.
                    let l_hi = self.fresh_label();
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    BTFSC 0x{:02X}, 0", self.scratch));
                    self.emit(format!("    GOTO {l_hi}"));
                    self.emit(format!("    MOVLW PAGE(__read_{name})"));
                    self.emit("    MOVWF PCLATH".to_string());
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    CALL __read_{name}"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit_pclath_restore(&format!("__read_{name}"));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                    self.emit(format!("    GOTO {l_done}"));
                    self.emit(format!("{l_hi}:"));
                    self.emit(format!("    MOVLW PAGE(__read_{name}_hi)"));
                    self.emit("    MOVWF PCLATH".to_string());
                    self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                    self.emit(format!("    CALL __read_{name}_hi"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit_pclath_restore(&format!("__read_{name}_hi"));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                } else {
                    emit_chain(self, &l_done);
                }
                self.emit(format!("{l_done}:"));
            }
            [] => panic!(
                "isel: constant index into large const table @{name} not supported (size > 255); only a single 16-bit reg index is supported"
            ),
            _ => panic!(
                "isel: multi-term index into large const table @{name} not supported: {terms:?}"
            ),
        }
    }

    /// W = byte `idx` of `val`.
    fn emit_load_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                if let Some((base, k, terms)) =
                    self.resolved.get(&ssa_key(self.cur_func, r)).cloned()
                {
                    // The shapes below read the base's two bytes as a runtime
                    // address. Only a pointer param's slot holds one: a global's
                    // or an alloca's address is a link-time constant that would
                    // need a literal materialization instead.
                    let sa = match &base {
                        Base::Slot(sname, false) if self.param_holds_addr(sname) => {
                            self.slot_addr(self.cur_func, sname).direct()
                        }
                        other => panic!("isel: cannot take the value of a GEP over {other:?}"),
                    };
                    // The pointer value is `base + k + Σterms`, so byte 1 needs
                    // the carry OUT of byte 0. Byte 0 only produces one when it
                    // actually adds something: a bare `MOVF` leaves the caller's
                    // carry standing, and propagating that adds a phantom 1.
                    let adds_in_byte0 = k != 0 || !terms.is_empty();
                    assert!(
                        k == 0 || terms.is_empty(),
                        "isel: GEP with both a constant offset and dynamic terms \
                         loses the term's carry; not supported"
                    );
                    match terms.as_slice() {
                        [] => {
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                if k != 0 {
                                    self.emit(format!("    ADDLW 0x{k:02X}"));
                                }
                            } else {
                                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                if adds_in_byte0 {
                                    self.emit("    BTFSC STATUS, 0".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                }
                            }
                        }
                        [(1, reg)] => {
                            let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                            } else {
                                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                self.emit("    BTFSC STATUS, 0".to_string());
                                self.emit("    ADDLW 0x01".to_string());
                                self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                            }
                        }
                        _ => {
                            assert!(
                                terms.len() == 2 && terms.iter().all(|(sc, _)| *sc == 1),
                                "isel: multi-term GEP load with {terms:?} not supported"
                            );
                            let ra1 = self.val_addr(&Val::Reg(terms[0].1.clone())).direct();
                            let ra2 = self.val_addr(&Val::Reg(terms[1].1.clone())).direct();
                            if idx == 0 {
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra1:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra2:02X}, W"));
                            } else {
                                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                self.emit("    BTFSC STATUS, 0".to_string());
                                self.emit("    ADDLW 0x01".to_string());
                                self.emit(format!("    ADDWF 0x{:02X}, W", ra1 + 1));
                                self.emit(format!("    ADDWF 0x{:02X}, W", ra2 + 1));
                            }
                        }
                    }
                    return;
                }
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit(format!("    MOVF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone())).direct();
                self.emit(format!("    MOVF 0x{:02X}, W", a + u16::from(idx)));
            }
        }
    }

    /// W ^= byte `idx` of `val`.
    fn emit_xor_byte(&mut self, val: &Val, idx: u8) {
        match val {
            Val::Const(k) => {
                let b = ((k >> (idx as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    XORLW 0x{b:02X}"));
            }
            Val::Reg(r) => {
                let a = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone())).direct();
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
        }
    }

    /// Copy `val` (width `ty`) into the slot starting at `dst`.
    fn emit_move_val_to_slot(&mut self, val: &Val, ty: Ty, dst: u16) {
        for i in 0..ty.bytes() {
            self.emit_load_byte(val, i);
            self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
        }
    }

    /// Set the Z flag to (a == b) without disturbing other flags. For
    /// multi-byte widths (i16/i32), the XORs of every byte pair are
    /// accumulated in the fixed `scratch` byte, leaving Z set exactly when
    /// every byte was equal.
    fn emit_cmp_eq(&mut self, a: &Val, b: &Val, ty: Ty) {
        let n = ty.bytes();
        self.emit_load_byte(a, 0);
        self.emit_xor_byte(b, 0);
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        for i in 1..n {
            self.emit_load_byte(a, i);
            self.emit_xor_byte(b, i);
            self.emit(format!("    IORWF 0x{:02X}, W", self.scratch));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        }
    }

    /// W = byte `i` of `v`, with the sign bit complemented (XOR 0x80) when
    /// `signed` and `i` is the high (sign) byte. Complementing the sign bit
    /// maps signed order onto unsigned order: signed(a >= b) ==
    /// unsigned((a ^ 0x80) >= (b ^ 0x80)), so one flag recipe serves both.
    fn emit_load_cmp_byte(&mut self, v: &Val, i: u8, signed: bool, high: u8) {
        match v {
            Val::Const(k) => {
                let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                let b = if signed && i == high {
                    byte ^ 0x80
                } else {
                    byte
                };
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            _ => {
                let addr = self.val_addr(v).direct() + u16::from(i);
                if signed && i == high {
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit(format!("    XORWF 0x{addr:02X}, W"));
                } else {
                    self.emit(format!("    MOVF 0x{addr:02X}, W"));
                }
            }
        }
    }

    /// Set C = (a >= b), unsigned or signed (sign-bit complement). For i8
    /// the SUBWF/SUBLW also leaves Z = (a == b); for i16/i32 the borrow
    /// chain's final Z is only a byte-level flag, so predicates needing
    /// equality append `emit_cmp_eq` (which preserves C). Every multi-byte
    /// width (i16 and i32) routes through the byte-generic wide emitters
    /// (`emit_cmp_c_file_lhs_wide` / `emit_cmp_c_const_lhs_wide`), which
    /// fold the borrow with the wrap-correct INCFSZ skip (issue #1); only
    /// the single-byte i8 path stays here. A const RHS becomes the
    /// MOVLW/SUBWF subtrahend; a const LHS uses SUBLW (k - W) since a const
    /// can never be read as a file register.
    fn emit_cmp_c(&mut self, a: &Val, b: &Val, ty: Ty, signed: bool) {
        let n = ty.bytes();
        let high = n - 1;
        match (a, b) {
            (Val::Const(_), Val::Const(_)) => panic!("isel: constant folding not implemented"),
            (Val::Const(k), _) => {
                if n > 1 {
                    // The multi-byte chains need the wrap-correct borrow
                    // folds (the naive ADDLW 1 fold corrupts the borrow-out
                    // at b_i = 0xFF + borrow-in); the i8 path below stays
                    // byte-identical (a single byte has no borrow chain).
                    self.emit_cmp_c_const_lhs_wide(k, b, n, high, signed);
                    return;
                }
                // SUBLW chain: W holds the b byte (+ borrow); SUBLW subtracts
                // it from the const byte, so C = (a >= b).
                self.emit_load_cmp_byte(b, 0, signed, high);
                let k0 = (k & 0xFF) as u8;
                // The low byte is the sign byte for i8: fold the complement
                // in when signed.
                let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
                self.emit(format!("    SUBLW 0x{k0:02X}"));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit("    ADDLW 0x01".to_string());
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    let kb = if signed && i == high { kb ^ 0x80 } else { kb };
                    self.emit(format!("    SUBLW 0x{kb:02X}"));
                }
            }
            _ => {
                if n > 1 {
                    self.emit_cmp_c_file_lhs_wide(a, b, n, high, signed);
                    return;
                }
                let aa = self.val_addr(a).direct();
                let use_scratch = signed; // signed file-LHS: SUBWF's file operand must be a ^ 0x80
                if use_scratch {
                    // Pre-store the complemented sign byte; MOVLW/XORWF/MOVWF
                    // do not touch C, and the low-byte SUBWF below sets it.
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit(format!("    XORWF 0x{:02X}, W", aa + high as u16));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                }
                self.emit_load_cmp_byte(b, 0, signed, high);
                self.emit(format!(
                    "    SUBWF 0x{:02X}, W",
                    if use_scratch && n == 1 {
                        self.scratch
                    } else {
                        aa
                    }
                ));
                for i in 1..n {
                    self.emit_load_cmp_byte(b, i, signed, high);
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit("    ADDLW 0x01".to_string());
                    let f = if i == high && use_scratch {
                        self.scratch
                    } else {
                        aa + i as u16
                    };
                    self.emit(format!("    SUBWF 0x{f:02X}, W"));
                }
            }
        }
    }

    /// The multi-byte (n > 1, i16 and i32) borrow chain for `C = (a >= b)`
    /// with a file-LHS `a`. The chain's intermediate borrow-outs are
    /// load-bearing, and the naive `ADDLW 1` fold corrupts the
    /// borrow-out exactly when the folded subtrahend wraps (b_i = 0xFF +
    /// borrow-in = 0x100): the SUBWF then sees W = 0 and leaves
    /// C = (a_i >= 0) = 1: a false "no borrow" that mis-compares every
    /// higher byte. Folding via INCFSZ's skip keeps C = borrow-in, the true
    /// borrow-out, and the skipped SUBWF's garbage W result is discarded
    /// (a cmp leaves only flags; a and b are never written, so INCFSZ can
    /// fold directly on the operand byte). The signed sign-complement
    /// applies to the HIGH byte only: the a-side is XORed 0x80 into the
    /// scratch (the SUBWF file operand), and the b-side is complemented
    /// into the 0x71 temp and folded *complemented* via INCFSZ's skip:
    /// the complemented fold wraps at b_hi ^ 0x80 = 0xFF (b_hi = 0x7F +
    /// borrow), where the skip keeps C = borrow-in = 0, the true
    /// borrow-out. A fold on the uncomplemented byte would repair only the
    /// b_hi = 0xFF wrap; b_hi = 0x7F + borrow would wrap invisibly and
    /// corrupt the final C.
    fn emit_cmp_c_file_lhs_wide(&mut self, a: &Val, b: &Val, n: u8, high: u8, signed: bool) {
        let aa = self.val_addr(a).direct();
        // Byte 0 has no borrow-in; a single SUBWF leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    SUBWF 0x{aa:02X}, W"));
        for i in 1..n {
            if signed && i == high {
                // Both sides are complemented at the high byte. The b-side
                // is folded COMPLEMENTED via INCFSZ's skip (0x71 as a
                // second temp, no live retval during a compare), because
                // the complemented fold wraps at b_hi ^ 0x80 = 0xFF
                // (b_hi = 0x7F + borrow): the skip keeps C = borrow-in = 0,
                // the true borrow-out. A fold on the *uncomplemented* byte
                // would repair only the b_hi = 0xFF wrap; b_hi = 0x7F +
                // borrow would wrap invisibly and corrupt the final C.
                match b {
                    Val::Const(k) => {
                        let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                    }
                    _ => {
                        let addr = self.val_addr(b).direct() + u16::from(high);
                        self.emit("    MOVLW 0x80".to_string());
                        self.emit(format!("    XORWF 0x{addr:02X}, W"));
                    }
                }
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    MOVLW 0x80".to_string());
                self.emit(format!("    XORWF 0x{:02X}, W", aa + u16::from(high)));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                // The a-side complement clobbered W; reload the complemented
                // b-side before the fold.
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                self.emit(format!("    SUBWF 0x{:02X}, W", self.scratch));
            } else {
                match b {
                    Val::Const(k) => {
                        let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                        self.emit(format!("    MOVLW 0x{kb:02X}"));
                        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    }
                    _ => {
                        self.emit_load_cmp_byte(b, i, signed, high);
                        let addr = self.val_addr(b).direct() + u16::from(i);
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{addr:02X}, W"));
                    }
                }
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + u16::from(i)));
            }
        }
    }

    /// The multi-byte (n > 1, i16 and i32) const-LHS (SUBLW) borrow chain:
    /// W holds the b byte (+ borrow), SUBLW subtracts it from the const
    /// byte. Same wrap-correct folds as `emit_cmp_c_file_lhs_wide`; the
    /// signed high byte's literal is pre-complemented (folded into the
    /// SUBLW operand) and the b-side is complemented into the 0x71 temp and
    /// folded COMPLEMENTED via INCFSZ's skip: the complemented fold
    /// wraps at b_hi ^ 0x80 = 0xFF (b_hi = 0x7F + borrow), where the
    /// skip keeps C = borrow-in, the true borrow-out.
    fn emit_cmp_c_const_lhs_wide(&mut self, k: &i64, b: &Val, n: u8, high: u8, signed: bool) {
        // Byte 0 has no borrow-in; a single SUBLW leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        let k0 = (k & 0xFF) as u8;
        let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
        self.emit(format!("    SUBLW 0x{k0:02X}"));
        for i in 1..n {
            if signed && i == high {
                // b is a reg/global here: complement it, stash in the 0x71
                // temp, and fold COMPLEMENTED via INCFSZ's skip (see
                // emit_cmp_c_file_lhs_wide: the complemented fold wraps at
                // b_hi = 0x7F + borrow, where the skip keeps the true
                // borrow-out).
                let addr = self.val_addr(b).direct() + u16::from(high);
                self.emit("    MOVLW 0x80".to_string());
                self.emit(format!("    XORWF 0x{addr:02X}, W"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                self.emit(format!("    SUBLW 0x{kb:02X}"));
            } else {
                let addr = self.val_addr(b).direct() + u16::from(i);
                self.emit(format!("    MOVF 0x{addr:02X}, W"));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{addr:02X}, W"));
                let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                self.emit(format!("    SUBLW 0x{kb:02X}"));
            }
        }
    }

    /// Materialize a flag predicate into `dst` as an i1. `cond` reads the C
    /// and/or Z flags left by the immediately preceding compare/accumulation
    /// (only MOVF/MOVLW/MOVWF/XORWF/XORLW/IORWF between, which never touch
    /// C). `Z` is the eq materialization; `!Z` (ne) inverts it; `C`/`!C`
    /// materialize uge/ult (and sge/slt); `C&&!Z`/`!C||Z` materialize
    /// ugt/ule (and sgt/sle).
    fn emit_materialize(&mut self, cond: &str, dst: u16) {
        let (skip, adj2) = match cond {
            "Z" => ("BTFSC STATUS, 2 ; Z", ""),
            "!Z" => ("BTFSS STATUS, 2 ; Z", ""),
            "C" => ("BTFSC STATUS, 0 ; C", ""),
            "!C" => ("BTFSS STATUS, 0 ; C", ""),
            "C&&!Z" => ("BTFSC STATUS, 0 ; C", "MOVLW 0x00"),
            "!C||Z" => ("BTFSS STATUS, 0 ; C", "MOVLW 0x01"),
            _ => panic!("isel: bad materialize cond {cond}"),
        };
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    {skip}"));
        self.emit("    MOVLW 0x01".to_string());
        if !adj2.is_empty() {
            // Second condition: C&&!Z clears the 1 when Z is set (equal);
            // !C||Z sets it when Z is set. BTFSC STATUS,2 skips the
            // adjustment when Z is clear.
            self.emit("    BTFSC STATUS, 2 ; Z".to_string());
            self.emit(format!("    {adj2}"));
        }
        self.emit(format!("    MOVWF 0x{dst:02X}"));
    }

    /// Branch on `cond`: Z = (cond == 0); if Z is set (cond == 0) go to `f`,
    /// otherwise (cond != 0) go to `t`. Mirrors spike emit_cond_branch.
    fn emit_cond_branch(&mut self, cond: &Val, t: &str, f: &str) {
        match cond {
            Val::Reg(r) => {
                let ca = self.val_addr(&Val::Reg(r.clone())).direct();
                self.emit(format!("    MOVF 0x{ca:02X}, W"));
                self.emit("    BTFSC STATUS, 2 ; Z".to_string());
                self.emit(format!("    GOTO {f}"));
                self.emit(format!("    GOTO {t}"));
            }
            Val::Const(k) => {
                let l = if *k != 0 { t } else { f };
                self.emit(format!("    GOTO {l}"));
            }
            Val::Global(_) => panic!("isel: conditional branch on a global"),
        }
    }

    /// `d = cond ? a : b` via an if/else jump over two copies. Mirrors spike
    /// emit_select.
    fn emit_select(&mut self, dst: &str, cond: &Val, ty: Ty, a: &Val, b: &Val) {
        let da = self.slot_addr(self.cur_func, dst).direct();
        match cond {
            Val::Const(k) => {
                let v = if *k != 0 { a } else { b };
                self.emit_move_val_to_slot(v, ty, da);
                return;
            }
            Val::Global(_) => panic!("isel: select condition is a global"),
            Val::Reg(_) => {}
        }
        let l_else = self.fresh_label();
        let l_end = self.fresh_label();
        let ca = match cond {
            Val::Reg(r) => self.val_addr(&Val::Reg(r.clone())).direct(),
            _ => unreachable!(),
        };
        self.emit(format!("    MOVF 0x{ca:02X}, W"));
        self.emit("    BTFSC STATUS, 2 ; Z".to_string());
        self.emit(format!("    GOTO {l_else}"));
        self.emit_move_val_to_slot(a, ty, da);
        self.emit(format!("    GOTO {l_end}"));
        self.emit(format!("{l_else}:"));
        self.emit_move_val_to_slot(b, ty, da);
        self.emit(format!("{l_end}:"));
    }

    /// A fresh local label for intra-block jumps (select branches). The
    /// counter lives at module scope so labels are unique across functions.
    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.tmp);
        *self.tmp += 1;
        s
    }

    /// `d = a + b` for i16 (either operand may be a register; at most one a
    /// const). Low byte adds, then the high byte adds with the carry from the
    /// low byte folded in via BTFSC/ADDLW.
    fn emit_add16(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add16 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVF 0x{ra:02X}, W"));
                self.emit(format!("    ADDLW 0x{lo:02X}"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", ra + 1));
                self.emit("    BTFSC STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    ADDLW 0x{hi:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(_) => panic!("isel: add16 with a global operand"),
        }
    }

    /// `d = a OP b` bytewise, for the commutative binops and/or/xor at i8 or
    /// i16. One operand is a register (the file operand), the other a
    /// register or const; a const LHS is swapped to the RHS so the literal
    /// path (`opw`) is used, never reading a const as a file-register
    /// address. `op` is the reg-file mnemonic (`ANDWF`/`IORWF`/`XORWF`),
    /// `opw` the literal mnemonic (`ANDLW`/`IORLW`/`XORLW`).
    fn emit_commutative(&mut self, a: &Val, b: &Val, ty: Ty, dst: u16, op: &str, opw: &str) {
        let n = ty.bytes();
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: {op} needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                for i in 0..n {
                    self.emit(format!("    MOVF 0x{:02X}, W", bb + u16::from(i)));
                    self.emit(format!("    {op} 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                }
            }
            Val::Const(k) => {
                for i in 0..n {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVF 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    {opw} 0x{byte:02X}"));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                }
            }
            Val::Global(_) => panic!("isel: {op} with a global operand"),
        }
    }

    /// `d = a - b` for i8: the subtrahend (reg or const) goes in W and SUBWF
    /// subtracts it from the minuend: SUBWF f,W always computes f - W, so
    /// `a` is the file operand. A const LHS is rejected by the caller (sub
    /// is not commutative).
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                // Mask the byte: clang prints an i8 constant >= 128 as a
                // negative i8 (e.g. `sub i8 %a, -42` for `a - 214u`), which
                // is the same value mod 256 (found by the fuzz corpus).
                self.emit(format!("    MOVLW 0x{:02X}", (*k & 0xFF) as u8));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b).direct();
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = k - a` (const LHS) for `bytes`-wide values. Byte 0 uses SUBLW
    /// (W = k - W, C exact, no borrow-in). Each higher byte folds the
    /// borrow with the wrap-correct INCFSZ idiom (ported from the i32
    /// chains, issue #1): the minuend byte `k_i` is preloaded into the
    /// destination, the subtrahend byte `a_i` is copied to the scratch, and
    /// `SUBWF dst_i, F` computes `k_i - (a_i + borrow)` in place. When the
    /// fold wraps (a_i = 0xFF + borrow-in = 0x100) the INCFSZ skip leaves
    /// the destination at `k_i`, the correct mod-256 result, with C =
    /// borrow-in, the true borrow-out. The naive `ADDLW 1` fold this
    /// replaces corrupted the borrow-out at the wrap (W = 0x00, C = 1), so
    /// every higher byte mis-subtracted.
    fn emit_sub_const_lhs(&mut self, k: &i64, a: &Val, dst: u16, bytes: u8) {
        let aa = self.val_addr(a).direct();
        self.emit(format!("    MOVF 0x{aa:02X}, W"));
        self.emit(format!("    SUBLW 0x{:02X}", (k & 0xFF) as u8));
        self.emit(format!("    MOVWF 0x{dst:02X}"));
        for i in 1..bytes {
            let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
            // The subtrahend byte is copied to the scratch first (the dst
            // preload may overlay a), then the minuend byte is preloaded
            // into the destination, then the fold runs against the scratch.
            self.emit(format!("    MOVF 0x{:02X}, W", aa + u16::from(i)));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            self.emit(format!("    MOVLW 0x{kb:02X}"));
            self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
            self.emit("    BTFSS STATUS, 0 ; C".to_string());
            self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
            self.emit(format!("    SUBWF 0x{:02X}, F", dst + u16::from(i)));
        }
    }

    /// `d = a - b` for i16: low byte SUBWF, then the high byte with the
    /// borrow from the low byte folded in: if C is clear (borrow), ADDLW 1
    /// bumps the subtrahend byte before the high SUBWF.
    fn emit_sub16(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                let lo = (k & 0xFF) as u8;
                let hi = ((k >> 8) & 0xFF) as u8;
                self.emit(format!("    MOVLW 0x{lo:02X}"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVLW 0x{hi:02X}"));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", bb + 1));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(_) => panic!("isel: sub16 with a global operand"),
        }
    }

    /// `d = a + b` for i32: byte 0 adds with the carry out exact (ADDWF),
    /// then each higher byte folds the carry into a scratch copy of the
    /// addend and accumulates into the destination in place. The fold uses
    /// INCFSZ's skip rather than the i16 chain's `ADDLW 1`: when the fold
    /// wraps (b_i = 0xFF + carry-in = 0x100) the skip leaves the
    /// destination at `a_i`, the correct mod-256 result, with C =
    /// carry-in, the true carry-out. The i16 fold's C would be corrupted at
    /// an intermediate byte (`SUBWF`-style re-derivation gives
    /// C = (a_i >= 0) = 1 there), silently mis-adding every higher byte.
    fn emit_add32(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add32 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg)).direct();
        // Byte 0: no carry-in; the ADDWF's C is exact.
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                for i in 1..4u8 {
                    // b_i is copied to scratch first (the dst preload may
                    // overlay b), then W is reloaded from it after the
                    // preload's MOVF clobbers W.
                    self.emit(format!("    MOVF 0x{:02X}, W", bb + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit(format!("    MOVF 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                    self.emit("    BTFSC STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    self.emit(format!("    ADDWF 0x{:02X}, F", dst + u16::from(i)));
                }
            }
            Val::Const(k) => {
                self.emit(format!("    MOVF 0x{ra:02X}, W"));
                self.emit(format!("    ADDLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                for i in 1..4u8 {
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVF 0x{:02X}, W", ra + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                    self.emit(format!("    MOVLW 0x{kb:02X}"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit("    BTFSC STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    self.emit(format!("    ADDWF 0x{:02X}, F", dst + u16::from(i)));
                }
            }
            Val::Global(_) => panic!("isel: add32 with a global operand"),
        }
    }

    /// `d = a - b` for i32: byte 0 subtracts with the borrow out exact
    /// (SUBWF), then each higher byte folds the borrow into a scratch copy
    /// of the subtrahend and subtracts from the destination in place. The
    /// fold uses INCFSZ's skip rather than the i16 chain's `ADDLW 1`: when
    /// the fold wraps (b_i = 0xFF + borrow-in = 0x100) the skip leaves the
    /// destination at `a_i`, the correct mod-256 result, with C =
    /// borrow-in = 0, the true borrow-out. The i16 fold's C would be
    /// corrupted at an intermediate byte (C = (a_i >= 0) = 1), silently
    /// mis-subtracting every higher byte.
    fn emit_sub32(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                for i in 1..4u8 {
                    let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVF 0x{:02X}, W", aa + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                    self.emit(format!("    MOVLW 0x{kb:02X}"));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    self.emit(format!("    SUBWF 0x{:02X}, F", dst + u16::from(i)));
                }
            }
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone())).direct();
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
                for i in 1..4u8 {
                    // b_i is copied to scratch first (the dst preload may
                    // overlay b), then W is reloaded from it after the
                    // preload's MOVF clobbers W.
                    self.emit(format!("    MOVF 0x{:02X}, W", bb + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
                    self.emit(format!("    MOVF 0x{:02X}, W", aa + u16::from(i)));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
                    self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
                    self.emit("    BTFSS STATUS, 0 ; C".to_string());
                    self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
                    self.emit(format!("    SUBWF 0x{:02X}, F", dst + u16::from(i)));
                }
            }
            Val::Global(_) => panic!("isel: sub32 with a global operand"),
        }
    }

    /// `dst = call func(args)`: copy each arg into the callee's
    /// `{func}::{param}` slots, `CALL func`, then copy the retval slots
    /// (`retval_lo` .. `retval_lo + bytes - 1`, 0x71-0x74 for i32) into
    /// `dst`. Void calls skip the retval copy. Mirrors spike emit_call.
    fn emit_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
    ) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        for (i, arg) in args.iter().enumerate() {
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(func, pname).direct();
            if let Some(size) = arg.byval {
                // byval: copy `size` bytes from the arg's pointer (global /
                // alloca slot / GEP reg) into the callee's param slot: the
                // param slot IS the callee's struct copy (Slot(name, false)),
                // byte by byte through the shared pointer machinery.
                assert_eq!(
                    size,
                    callee.params[i]
                        .byval
                        .expect("isel: byval arg for a non-byval param"),
                    "isel: byval size mismatch for arg {i} of @{func}"
                );
                for b in 0..size {
                    self.emit_ptr_load_byte(&arg.val, b);
                    self.emit(format!("    MOVWF 0x{:02X}", pa + u16::from(b)));
                }
            } else if arg.sret {
                // sret: store the target address into the callee's sret param
                // slot (2 bytes). The target is a global or a plain alloca
                // slot; the callee reaches it through FSR+IRP, so the target
                // object must fit entirely inside one GPR window: a span
                // crossing an SFR hole would silently mis-address (the same
                // loud rule as static FSR bases). The MOVLW LOW/HIGH store
                // emits both address bytes unchanged.
                assert!(callee.params[i].sret, "isel: sret arg for a non-sret param");
                let (addr, span) = match &arg.val {
                    Val::Global(g) => (
                        self.global_addr(g),
                        self.object_span(&Base::Global(g.clone())),
                    ),
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        assert!(
                            k == 0 && terms.is_empty(),
                            "isel: sret target must be a plain global or alloca slot (no offset)"
                        );
                        let addr = match &base {
                            Base::Global(name) => self.global_addr(name),
                            Base::Slot(sname, false) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                            Base::Slot(_, true) => {
                                panic!("isel: sret target cannot be an indirect (sret) slot")
                            }
                        };
                        let span = self.object_span(&base);
                        (addr, span)
                    }
                    Val::Const(_) => panic!("isel: sret target must be a global or an alloca slot"),
                };
                fsr_window(self.device, addr, span);
                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa));
                self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
            } else if arg.ty.is_none() {
                assert!(
                    !arg.sret && arg.byval.is_none(),
                    "isel: plain ptr arg must be non-sret/non-byval"
                );
                assert_eq!(
                    callee.params[i].width, 2,
                    "isel: callee ptr param must be 2 bytes"
                );
                match &arg.val {
                    Val::Global(g) => {
                        let addr = self.global_addr(g);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                    }
                    Val::Const(c) => {
                        assert_eq!(*c, 0, "isel: non-zero const ptr not supported");
                        self.emit(format!("    CLRF 0x{:02X}", pa));
                        self.emit(format!("    CLRF 0x{:02X}", pa + 1));
                    }
                    // A global at a constant offset (`&g[2]`) has a link-time
                    // address: materialize it as two literals.
                    Val::Reg(r) if matches!(self.resolved_for(r), (Base::Global(_), _, ref t) if t.is_empty()) =>
                    {
                        let (base, k, _) = self.resolved_for(r);
                        let Base::Global(name) = &base else {
                            unreachable!()
                        };
                        let addr = self.global_addr(name) + u16::from(k);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                    }
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        // As in `emit_load_byte`: the shapes below read the base
                        // slot's two bytes as a runtime address.
                        let sa = match &base {
                            Base::Slot(sname, false) if self.param_holds_addr(sname) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                            other => panic!("isel: cannot pass a GEP over {other:?} as a ptr arg"),
                        };
                        let k_lo = (u16::from(k) & 0xFF) as u8;
                        let k_hi = (u16::from(k) >> 8) as u8;
                        match terms.as_slice() {
                            [] => {
                                if k_lo == 0 && k_hi == 0 {
                                    self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                    self.emit(format!("    MOVWF 0x{:02X}", pa));
                                    self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                    self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                                } else {
                                    self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                    if k_lo != 0 {
                                        self.emit(format!("    ADDLW 0x{k_lo:02X}"));
                                    }
                                    self.emit(format!("    MOVWF 0x{:02X}", pa));
                                    self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                    if k_lo != 0 {
                                        self.emit(format!("    BTFSC STATUS, 0"));
                                        self.emit(format!("    ADDLW 0x01"));
                                    }
                                    if k_hi != 0 {
                                        self.emit(format!("    ADDLW 0x{k_hi:02X}"));
                                    }
                                    self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                                }
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                                self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                                if k_lo != 0 {
                                    self.emit(format!("    ADDLW 0x{k_lo:02X}"));
                                }
                                self.emit(format!("    MOVWF 0x{:02X}", pa));
                                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                                self.emit(format!("    BTFSC STATUS, 0"));
                                self.emit(format!("    ADDLW 0x01"));
                                self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                                if k_hi != 0 {
                                    self.emit(format!("    ADDLW 0x{k_hi:02X}"));
                                }
                                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                            }
                            _ => panic!("isel: plain ptr arg with multiple terms not yet supported: {terms:?}"),
                        }
                    }
                }
            } else {
                let aty = arg.ty.expect("isel: scalar call arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, aty, pa);
                // M15 conversion ABI: the four conversion routines take
                // their value in a fixed 4-byte `val` slot, but i8/i16
                // sources are copied by their own width: the leftover
                // high bytes are STALE and corrupt the recipe's leading-1
                // search / sign logic (an i16 `sitofp` reading leftover
                // high bytes produced exp 157 instead of 130 in the M15
                // acceptance). Fill them so the slot holds a proper i32:
                // __sitofp_f32 sign-extends (i16: the top value byte IS
                // the sign byte, copied up; i8: 0xFF/0x00 by bit 7),
                // __uitofp_f32 zero-extends.
                if aty.bytes() < callee.params[i].width {
                    assert_eq!(
                        callee.params[i].width, 4,
                        "isel: narrow scalar arg {i} of @{func} into a non-4-byte param"
                    );
                    let aw = aty.bytes() as u16;
                    match func {
                        "__uitofp_f32" => {
                            for j in aw..4 {
                                self.emit(format!("    CLRF 0x{:02X}", pa + j));
                            }
                        }
                        "__sitofp_f32" => {
                            let sign = pa + aw - 1;
                            if aw == 2 {
                                self.emit(format!("    MOVF 0x{sign:02X}, W"));
                                self.emit(format!("    MOVWF 0x{:02X}", pa + 2));
                                self.emit(format!("    MOVWF 0x{:02X}", pa + 3));
                            } else {
                                assert_eq!(
                                    aw, 1,
                                    "isel: unexpected narrow source width for @{func}"
                                );
                                self.emit("    MOVLW 0x00".to_string());
                                self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
                                self.emit("    MOVLW 0xFF".to_string());
                                for j in 1..4 {
                                    self.emit(format!("    MOVWF 0x{:02X}", pa + j));
                                }
                            }
                        }
                        other => panic!("isel: narrow scalar arg into the wide param of @{other}"),
                    }
                }
            }
        }
        // M11 PCLATH discipline: every CALL runs with PCLATH<4:3> = the
        // target's page. The set's MOVLW clobbers W, so it must come AFTER
        // the last arg copy (which uses W) and immediately before the CALL;
        // the caller's own page is restored right after, unless the target
        // is in the caller's own page, where the restore is skipped (PCLATH
        // still holds the caller's page after the call, so its
        // intra-function GOTOs keep branching in its page).
        self.emit(format!("    MOVLW PAGE({func})"));
        self.emit("    MOVWF PCLATH".to_string());
        self.emit(format!("    CALL {func}"));
        self.emit_pclath_restore(func);
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            // Copy the retval region (0x71..0x71+bytes-1, up to 0x74 for
            // i32) into dst.
            let da = self.slot_addr(self.cur_func, d).direct();
            for i in 0..t.bytes() {
                self.emit(format!(
                    "    MOVF 0x{:02X}, W",
                    self.retval_lo + u16::from(i)
                ));
                self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
            }
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                if let Some(g) = l.ptr.strip_prefix('@') {
                    let src = self.global_addr(g);
                    for k in 0..l.ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(k)));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                } else if l.ptr.starts_with("0x") {
                    // A literal (SFR) pointer from `inttoptr`: a direct MOVF
                    // with no FSR setup. The banking pass supplies whatever
                    // BANKSEL the address turns out to need.
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", base + u16::from(k)));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                } else {
                    // A GEP-created pointer: const (flash) bases take the
                    // RETLW path (each byte a table read, multi-byte loads
                    // loop over the bytes); RAM bases go through the shared
                    // byte machinery (direct or FSR/INDF).
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global, %reg or a literal", l.ptr)
                    });
                    let ptr = Val::Reg(r.to_string());
                    for k in 0..l.ty.bytes() {
                        self.emit_ptr_load_byte(&ptr, k);
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                }
            }
            Inst::Store(s) => {
                assert!(s.ty != Ty::I1, "isel: only i8/i16 stores supported");
                if let Some(g) = s.ptr.strip_prefix('@') {
                    let dst = self.global_addr(g);
                    self.emit_move_val_to_slot(&s.val, s.ty, dst);
                } else if s.ptr.starts_with("0x") {
                    // A literal (SFR) pointer from `inttoptr`: a direct MOVWF
                    // with no FSR setup, banked by the banking pass.
                    let base = literal_ptr_addr(&s.ptr);
                    for k in 0..s.ty.bytes() {
                        self.emit_load_byte(&s.val, k);
                        self.emit(format!("    MOVWF 0x{:02X}", base + u16::from(k)));
                    }
                } else {
                    let r = s.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global, %reg or a literal", s.ptr)
                    });
                    let (base, _, _) = self.resolved_for(r);
                    if let Base::Global(name) = &base {
                        assert!(
                            !self.global_is_const(name),
                            "isel: store to const (flash) global @{name}"
                        );
                    }
                    let ptr = Val::Reg(r.to_string());
                    for k in 0..s.ty.bytes() {
                        self.emit_ptr_store_byte(&ptr, k, &s.val);
                    }
                }
            }
            Inst::Gep(_) => {} // virtual: lowered at each load/store use
            Inst::Alloca(_) => {} // virtual: the slot is sized by alloc; lowered at each use
            Inst::Memcpy(m) => match &m.len {
                MemLen::Const(n) => {
                    // Byte loop over the same pointer machinery: src[i] ->
                    // dst[i]. Each byte re-resolves both pointers (dst
                    // itself may be a base+k+i expression), exactly like a
                    // per-byte load/store.
                    for i in 0..*n {
                        self.emit_ptr_load_byte(&m.src, i);
                        self.emit_ptr_store_w(&m.dst, i);
                    }
                }
                MemLen::Reg(v) => self.emit_memcpy_dynamic(&m.dst, &m.src, v),
            },
            Inst::Bin(b) => {
                assert!(b.ty != Ty::I1 || matches!(b.op, BinOp::And | BinOp::Or | BinOp::Xor), "isel: only i8/i16/i32 binops supported (and i1 And/Or/Xor)");
                let b_ty = if b.ty == Ty::I1 { Ty::I8 } else { b.ty };
                let da = self.slot_addr(self.cur_func, &b.dst).direct();
                match (b.op, b_ty) {
                    (BinOp::Add, Ty::I16) => self.emit_add16(&b.a, &b.b, da),
                    (BinOp::Add, Ty::I32) => self.emit_add32(&b.a, &b.b, da),
                    (BinOp::Add, Ty::I8) => {
                        // Normalize commutative add: a const LHS is swapped to
                        // the RHS so the const-adder arm is used, never reading
                        // a const as a file-register address.
                        let (a, b_op) = match (&b.a, &b.b) {
                            (Val::Const(_), Val::Const(_)) => {
                                panic!("isel: constant folding not implemented")
                            }
                            (Val::Const(_), _) => (&b.b, &b.a),
                            _ => (&b.a, &b.b),
                        };
                        match b_op {
                            Val::Const(k) => {
                                // Mask to the byte (negative i8 constants,
                                // found by the fuzz corpus).
                                let kb = (*k & 0xFF) as u8;
                                let aa = self.val_addr(a).direct();
                                self.emit(format!("    MOVF 0x{aa:02X}, W"));
                                self.emit(format!("    ADDLW 0x{kb:02X}"));
                                self.emit(format!("    MOVWF 0x{da:02X}"));
                            }
                            _ => {
                                let (aa, bb) = (self.val_addr(a).direct(), self.val_addr(b_op).direct());
                                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                                self.emit(format!("    ADDWF 0x{aa:02X}, W"));
                                self.emit(format!("    MOVWF 0x{da:02X}"));
                            }
                        }
                    }
                    // Commutative bytewise binops (and/or/xor) share one
                    // emitter for both widths; a const LHS is swapped to the
                    // RHS by emit_commutative.
                    (BinOp::And, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::And, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::And, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::Or, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Or, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Or, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Xor, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    (BinOp::Xor, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    (BinOp::Xor, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    // sub is NOT commutative: a const LHS (d = k - a) cannot
                    // reuse the reg-const lowering (which computes a - k):
                    // SUBLW k computes k - W, so the const-LHS path mirrors
                    // the reg-const borrow chain with the roles swapped
                    // (found by the fuzz corpus; a generated `k - a` shape).
                    (BinOp::Sub, Ty::I8) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 1);
                        } else {
                            self.emit_sub8(&b.a, &b.b, da);
                        }
                    }
                    (BinOp::Sub, Ty::I16) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 2);
                        } else {
                            self.emit_sub16(&b.a, &b.b, da);
                        }
                    }
                    (BinOp::Sub, Ty::I32) => {
                        if let Val::Const(k) = &b.a {
                            self.emit_sub_const_lhs(k, &b.b, da, 4);
                        } else {
                            self.emit_sub32(&b.a, &b.b, da);
                        }
                    }
                    // Milestone-8 binops: legalize rewrites every mul/div/rem
                    // into a runtime routine call, so these ops reach isel only
                    // via hand-written IR. Panic loudly: the invariant that a
                    // legalize miss never silently miscompiles.
                    (BinOp::Mul, _) => panic!("isel: mul reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::UDiv, _) => panic!("isel: udiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::URem, _) => panic!("isel: urem reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SDiv, _) => panic!("isel: sdiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SRem, _) => panic!("isel: srem reached isel; legalize must rewrite it to a routine call"),
                    // Milestone-8 shifts: a const count inlines as a fixed
                    // RLF/RRF sequence; k == 0 is a plain copy; k >= width
                    // is LLVM poison and panics loudly. A variable (reg)
                    // count must never reach isel: legalize rewrites it to
                    // the routine call, so one arriving here is a legalize
                    // regression and panics loudly too.
                    (BinOp::Shl, _) | (BinOp::LShr, _) | (BinOp::AShr, _) => {
                        let width = b.ty.bytes() as i64 * 8;
                        let k = match &b.b {
                            Val::Const(k) => *k,
                            other => panic!(
                                "isel: variable-count {:?} shift reached isel (count {other:?}); legalize must rewrite it to a routine call",
                                b.op
                            ),
                        };
                        assert!(
                            (0..width).contains(&k),
                            "isel: const shift count {k} out of range [0, {width}) (LLVM poison)"
                        );
                        // Copy the value into the dst slot, then rotate the
                        // dst in place k times. shl: lo then hi (carry goes
                        // up); lshr: hi then lo (bits come down); ashr: set C
                        // from the sign bit before each rrf so the sign fills
                        // every vacated bit.
                        self.emit_move_val_to_slot(&b.a, b.ty, da);
                        let n = b.ty.bytes();
                        for _ in 0..k {
                            match b.op {
                                BinOp::Shl => {
                                    self.emit("    BCF STATUS, 0");
                                    for i in 0..n {
                                        self.emit(format!(
                                            "    RLF 0x{:02X}, F",
                                            da + u16::from(i)
                                        ));
                                    }
                                }
                                BinOp::LShr => {
                                    self.emit("    BCF STATUS, 0");
                                    for i in (0..n).rev() {
                                        self.emit(format!(
                                            "    RRF 0x{:02X}, F",
                                            da + u16::from(i)
                                        ));
                                    }
                                }
                                BinOp::AShr => {
                                    let hi = da + u16::from(n - 1);
                                    self.emit(format!("    BTFSC 0x{hi:02X}, 7"));
                                    self.emit("    BSF STATUS, 0");
                                    self.emit(format!("    BTFSS 0x{hi:02X}, 7"));
                                    self.emit("    BCF STATUS, 0");
                                    for i in (0..n).rev() {
                                        self.emit(format!(
                                            "    RRF 0x{:02X}, F",
                                            da + u16::from(i)
                                        ));
                                    }
                                }
                                _ => unreachable!(),
                            }
                        }
                    }
                    _ => panic!("isel: unsupported binop for milestone 2"),
                }
            }
            // freeze is a no-op in the backend: copy `val` byte-for-byte into
            // the dst slot (same shape as emit_move_val_to_slot).
            Inst::Freeze(f) => {
                let da = self.slot_addr(self.cur_func, &f.dst).direct();
                self.emit_move_val_to_slot(&f.val, f.ty, da);
            }
            Inst::Zext(z) => {
                // `zext i1 to i8` is legal and common (`u8 b = (a < b);`):
                // i1 and i8 are both 1 byte in the byte model, and an icmp
                // result is materialized as a byte holding exactly 0/1, so
                // a 1-byte copy IS the zext. Equal-width iN -> iN is zext
                // identity; only narrowing (i16/i32 -> i8) is a real error.
                assert!(
                    z.from.bytes() <= z.to.bytes(),
                    "isel: zext must not narrow"
                );
                let da = self.slot_addr(self.cur_func, &z.dst).direct();
                for i in 0..z.from.bytes() {
                    self.emit_load_byte(&z.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                for i in z.from.bytes()..z.to.bytes() {
                    self.emit(format!("    CLRF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Sext(x) => {
                // i8/i16 -> i16/i32, sign-filling from the SOURCE's high
                // byte (the loop below reads `x.from.bytes() - 1`). i1 has
                // no meaningful sign bit (a 0/1 value), so i1 -> iN panics
                // loudly rather than bit-7 sign-filling a non-sign.
                assert!(
                    x.from != Ty::I1 && x.from.bytes() < x.to.bytes(),
                    "isel: sext only supports i8/i16 -> i16/i32 (i1 sign-fill is undefined)"
                );
                assert!(
                    !matches!(&x.val, Val::Const(_)),
                    "isel: sext of a constant not supported (constant folding not implemented)"
                );
                let da = self.slot_addr(self.cur_func, &x.dst).direct();
                // Copy the low bytes unchanged.
                for i in 0..x.from.bytes() {
                    self.emit_load_byte(&x.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                // Fill the high bytes with the source's sign bit: test the
                // MSB of the source's high byte, then MOVLW 0xFF (set) or
                // 0x00 (clear) once and store it into every high byte.
                let src_hi = x.from.bytes() - 1;
                let a = self.val_addr(&x.val).direct();
                let l_pos = self.fresh_label();
                let l_fill = self.fresh_label();
                self.emit(format!("    BTFSS 0x{:02X}, 7", a + u16::from(src_hi)));
                self.emit(format!("    GOTO {l_pos}"));
                self.emit("    MOVLW 0xFF".to_string());
                self.emit(format!("    GOTO {l_fill}"));
                self.emit(format!("{l_pos}:"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit(format!("{l_fill}:"));
                for i in x.from.bytes()..x.to.bytes() {
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Trunc(t) => {
                // i1 and i8 are both one byte, so the byte widths alone do not
                // separate `trunc i8 -> i1` (narrowing) from a widening trunc.
                assert!(
                    t.from.bytes() > t.to.bytes() || (t.to == Ty::I1 && t.from != Ty::I1),
                    "isel: trunc must narrow"
                );
                let da = self.slot_addr(self.cur_func, &t.dst).direct();
                for i in 0..t.to.bytes() {
                    self.emit_load_byte(&t.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                if t.to == Ty::I1 {
                    // Every i1 consumer tests the whole byte for nonzero, so
                    // the truncated-away bits have to go: 0x02 is false.
                    self.emit("    MOVLW 0x01".to_string());
                    self.emit(format!("    ANDWF 0x{da:02X}, F"));
                }
            }
            Inst::Icmp(ic) => {
                let da = self.slot_addr(self.cur_func, &ic.dst).direct();
                match ic.pred.as_str() {
                    "eq" => {
                        // XOR-based compare sets Z = (a == b); materialize
                        // the i1. Kept byte-identical.
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("Z", da);
                    }
                    "ne" => {
                        // !Z: the eq compare with the inverted
                        // materialization (BTFSS instead of BTFSC).
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("!Z", da);
                    }
                    pred => {
                        let (signed, need_z) = match pred {
                            "ult" | "uge" => (false, false),
                            "ugt" | "ule" => (false, true),
                            "slt" | "sge" => (true, false),
                            "sgt" | "sle" => (true, true),
                            _ => panic!("isel: unknown icmp predicate {pred:?}"),
                        };
                        // C = (a >= b), unsigned or signed (sign-bit
                        // complement). i8 leaves Z = (a == b) too.
                        self.emit_cmp_c(&ic.a, &ic.b, ic.ty, signed);
                        // A multi-byte borrow chain ends with a byte-level
                        // Z; full equality needs the XOR accumulation
                        // (byte-generic across every width), which
                        // preserves C.
                        if need_z && ic.ty.bytes() > 1 {
                            self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        }
                        let mat = match pred {
                            "ult" | "slt" => "!C",
                            "uge" | "sge" => "C",
                            "ugt" | "sgt" => "C&&!Z",
                            _ => "!C||Z", // ule | sle
                        };
                        self.emit_materialize(mat, da);
                    }
                }
            }
            Inst::Select(s) => {
                self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
            }
            Inst::Call(c) => self.emit_call(&c.dst, c.ty, &c.func, &c.args),
            Inst::Asm(a) => {
                // Asm barrier: W/STATUS/bank clobbered — verbatim, bracketed for banking.
                // Rung 4: substitute `$0`/`%0` memory operands via slot_addr.
                self.emit("; --- asm start ---".to_string());
                let substituted = self.substitute_asm(&a.template, &a.operands);
                for line in substituted.split('\n') {
                    self.emit(line.to_string());
                }
                self.emit("; --- asm end ---".to_string());
            }
            Inst::FloatBin(_) | Inst::Fcmp(_) | Inst::FloatConv(_) => panic!(
                "isel: float instructions are not code-generated yet (Task 3 soft-float runtime routines)"
            ),
            _ => panic!("isel: unsupported instruction for milestone 2"),
        }
    }

    fn emit_terminator(&mut self, t: &Inst, labels: &HashMap<String, String>) {
        match t {
            Inst::Br(br) => {
                let l = &labels[&br.target];
                self.emit(format!("    GOTO {l}"));
            }
            Inst::BrCond(b) => {
                let lt = &labels[&b.t];
                let lf = &labels[&b.f];
                self.emit_cond_branch(&b.cond, lt, lf);
            }
            Inst::Ret(None) => self.emit("    RETURN".to_string()),
            Inst::Ret(Some((ty, v))) => {
                // Copy the value into the fixed retval slots (0x71..0x74 for
                // i32), then RETURN.
                for i in 0..ty.bytes() {
                    self.emit_load_byte(v, i);
                    self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + u16::from(i)));
                }
                self.emit("    RETURN".to_string());
            }
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }

    // ---- M8 Task 3: mul/div/rem runtime routine recipes ----

    /// Every recipe slot must sit inside ONE GPR bank (issue #6): the loops
    /// are skip-sensitive (BTFSS + GOTO, DECFSZ + GOTO, INCFSZ + ADDWF), so
    /// a BANKSEL the banking pass would insert between a test and its
    /// target, or between the two operands of a carry idiom, would change
    /// the skip targets. `alloc` rounds a routine's frame wholesale into a
    /// single bank; this verifies the placement (a silent straddle would
    /// miscompile, so it panics loudly instead).
    fn assert_bank0(&self, addrs: &[u16], routine: &str) {
        if addrs.is_empty() {
            return;
        }
        let first = addrs[0];
        let target = self.device.bank_of(first).unwrap_or_else(|| {
            panic!(
                "isel: {routine} slot 0x{first:02X} is not a banked GPR \
                 (recipe loops are skip-sensitive; a BANKSEL would change skip targets)"
            )
        });
        for &a in &addrs[1..] {
            let b = self.device.bank_of(a).unwrap_or_else(|| {
                panic!(
                    "isel: {routine} slot 0x{a:02X} is not a banked GPR \
                     (recipe loops are skip-sensitive; a BANKSEL would change skip targets)"
                )
            });
            assert!(
                b == target,
                "isel: {routine} slots straddle banks (0x{first:02X} bank {target}, \
                 0x{a:02X} bank {b}); recipe loops are skip-sensitive, a BANKSEL \
                 would change skip targets"
            );
        }
    }

    /// Copy `bytes` bytes from a routine slot into the fixed retval slots
    /// (0x71-0x74): `emit_call` on the caller side reads them after CALL.
    fn store_retval(&mut self, src: u16, bytes: u8) {
        for i in 0..bytes {
            self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(i)));
            self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + u16::from(i)));
        }
    }

    /// Two's-complement negate of a 16-bit value in place.
    fn neg16_in_place(&mut self, addr: u16) {
        self.emit(format!("    COMF 0x{addr:02X}, F"));
        self.emit(format!("    COMF 0x{:02X}, F", addr + 1));
        self.emit(format!("    INCF 0x{addr:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", addr + 1));
    }

    /// Two's-complement negate of a 32-bit value in place (the INCF carry
    /// propagates byte-by-byte through the Z chain).
    fn neg32_in_place(&mut self, addr: u16) {
        for i in 0..4 {
            self.emit(format!("    COMF 0x{:02X}, F", addr + i));
        }
        self.emit(format!("    INCF 0x{addr:02X}, F"));
        for i in 1..4 {
            self.emit("    BTFSC STATUS, 2".to_string());
            self.emit(format!("    INCF 0x{:02X}, F", addr + i));
        }
    }

    /// One 16-iteration chunk of the 32-iteration `__mul_u32` AN526 loop:
    /// test `bk`'s LSB, add `t` to `r` across all 4 bytes (the incfsz
    /// carry idiom), shift `t` left with wraparound (the shifted-out high
    /// bits are discarded, i32 `mul` wraps), shift `bk` right, count 16.
    fn emit_mul32_loop(
        &mut self,
        l_loop: String,
        l_skip: String,
        bk_lo: u16,
        bk_hi: u16,
        cnt: u16,
        r: [u16; 4],
        t: [u16; 4],
    ) {
        self.emit(format!("{l_loop}:"));
        self.emit(format!("    BTFSS 0x{bk_lo:02X}, 0")); // test multiplier LSB
        self.emit(format!("    GOTO {l_skip}"));
        self.emit(format!("    MOVF 0x{:02X}, W", t[0]));
        self.emit(format!("    ADDWF 0x{:02X}, F", r[0]));
        for i in 1..4 {
            self.emit(format!("    MOVF 0x{:02X}, W", t[i]));
            self.emit("    BTFSC STATUS, 0".to_string());
            self.emit(format!("    INCFSZ 0x{:02X}, W", t[i]));
            self.emit(format!("    ADDWF 0x{:02X}, F", r[i]));
        }
        self.emit(format!("{l_skip}:"));
        self.emit("    BCF STATUS, 0".to_string());
        for t_i in t {
            self.emit(format!("    RLF 0x{t_i:02X}, F")); // t <<= 1 (wrapping)
        }
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{bk_hi:02X}, F"));
        self.emit(format!("    RRF 0x{bk_lo:02X}, F")); // bk >>= 1
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
    }

    /// The 32-iteration restoring-division loop shared by the i32 divmod
    /// routines: `num` (the param slot) shifts left one bit per iteration
    /// (the quotient builds in its vacated bits), `rem`@0-3 accumulates the
    /// partial remainder, `den`@4-7 holds the denominator copy the
    /// subtract/restore chains read, `cnt`@8 counts 32 iterations. The
    /// full-width remainder never carries out of its 4 bytes for a 32/32
    /// divide (rem <= 2^k - 1 before the k-th shift), so the plain 4-byte
    /// borrow chain is exact: no extended-bit special case. C after the
    /// last SUBWF is 1 iff rem >= den (the quotient-bit discriminator).
    fn emit_divmod32(&mut self, num: u16, scr: u16) {
        let (rem0, den0, cnt) = (scr, scr + 4, scr + 8);
        let l_loop = self.fresh_label();
        let l_restore = self.fresh_label();
        let l_next = self.fresh_label();
        for i in 0..4 {
            self.emit(format!("    CLRF 0x{:02X}", rem0 + i));
        }
        self.emit("    MOVLW 0x20".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_loop}:"));
        self.emit("    BCF STATUS, 0".to_string());
        for i in 0..4 {
            self.emit(format!("    RLF 0x{:02X}, F", num + i));
        }
        for i in 0..4 {
            self.emit(format!("    RLF 0x{:02X}, F", rem0 + i));
        }
        // rem -= den across 4 bytes (the INCFSZ wrap-correct borrow folds);
        // C = (rem >= den) after the last byte.
        for i in 0..4 {
            self.emit(format!("    MOVF 0x{:02X}, W", den0 + i));
            if i == 0 {
                self.emit(format!("    SUBWF 0x{:02X}, F", rem0 + i));
            } else {
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den0 + i));
                self.emit(format!("    SUBWF 0x{:02X}, F", rem0 + i));
            }
        }
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_restore}"));
        self.emit(format!("    BSF 0x{num:02X}, 0"));
        self.emit(format!("    GOTO {l_next}"));
        self.emit(format!("{l_restore}:"));
        // rem += den (the exact add-back restore, carry folds).
        for i in 0..4 {
            self.emit(format!("    MOVF 0x{:02X}, W", den0 + i));
            if i == 0 {
                self.emit(format!("    ADDWF 0x{:02X}, F", rem0 + i));
            } else {
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den0 + i));
                self.emit(format!("    ADDWF 0x{:02X}, F", rem0 + i));
            }
        }
        self.emit(format!("{l_next}:"));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
    }

    /// The recipe body for one of the fifteen mul/div/rem runtime routines
    /// (i8/i16/i32), adapted from the machine-verified epicurus PIC16 asm
    /// (`epic_math_mul.c` AN526 shift-add; `epic_math_div.c` restoring
    /// shift-subtract). Args arrive in the routine's `{func}::{param}` slots
    /// (copied by `emit_call`), the result goes to the fixed retval slots,
    /// and working state lives in `{func}::__scr` at the layout-contract
    /// offsets. Plain addresses only: the banking pass inserts BANKSELs.
    /// Div-by-zero is LLVM poison: the loop runs (den = 0 ⇒ deterministic
    /// but arbitrary (poison)), any value is legal: no guard, documented. The nine
    /// shift routines (variable count) share `emit_shift_body`.
    fn emit_routine(&mut self) {
        // `name` addresses this function's OWN slots and label (an `_isr`
        // copy has its own frame); `recipe` selects the shared body.
        let name = self.cur_func;
        let recipe = routine_recipe(name)
            .unwrap_or_else(|| panic!("isel: @{name} is not a runtime routine"));
        let scr = self.slot_addr(name, "__scr").direct();
        self.emit(format!("{name}:"));
        match recipe {
            // Variable-count shifts: mask the count to width-1, bounded
            // loop over the val param slot (see emit_shift_body).
            "__shl_u8" | "__lshr_u8" | "__ashr_i8" | "__shl_u16" | "__lshr_u16" | "__ashr_i16"
            | "__shl_u32" | "__lshr_u32" | "__ashr_i32" => {
                let (bytes, op) = match recipe {
                    "__shl_u8" => (1, BinOp::Shl),
                    "__shl_u16" => (2, BinOp::Shl),
                    "__shl_u32" => (4, BinOp::Shl),
                    "__lshr_u8" => (1, BinOp::LShr),
                    "__lshr_u16" => (2, BinOp::LShr),
                    "__lshr_u32" => (4, BinOp::LShr),
                    "__ashr_i8" => (1, BinOp::AShr),
                    "__ashr_i16" => (2, BinOp::AShr),
                    "__ashr_i32" => (4, BinOp::AShr),
                    _ => unreachable!(),
                };
                self.emit_shift_body(bytes, op, scr);
            }
            // 8x8 -> 16 shift-add (AN526): t = a shifted left one bit per
            // multiplier bit; for each set bit of bk, r += t. Store the low
            // byte of the product (the i8 result).
            "__mul_u8" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, b, scr, scr + 5], name);
                let (bk, cnt, r_lo, r_hi, t_lo, t_hi) =
                    (scr, scr + 1, scr + 2, scr + 3, scr + 4, scr + 5);
                let l_loop = self.fresh_label();
                let l_skip = self.fresh_label();
                for r in [r_lo, r_hi, t_lo, t_hi] {
                    self.emit(format!("    CLRF 0x{r:02X}"));
                }
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    MOVWF 0x{t_lo:02X}")); // t = a
                self.emit(format!("    MOVF 0x{b:02X}, W"));
                self.emit(format!("    MOVWF 0x{bk:02X}")); // bk = b
                self.emit("    MOVLW 0x08".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}")); // cnt = 8
                self.emit(format!("{l_loop}:"));
                self.emit(format!("    BTFSS 0x{bk:02X}, 0")); // test multiplier LSB
                self.emit(format!("    GOTO {l_skip}"));
                self.emit(format!("    MOVF 0x{t_lo:02X}, W"));
                self.emit(format!("    ADDWF 0x{r_lo:02X}, F"));
                self.emit(format!("    MOVF 0x{t_hi:02X}, W"));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{t_hi:02X}, W")); // t_hi + carry; skip if wrapped
                self.emit(format!("    ADDWF 0x{r_hi:02X}, F"));
                self.emit(format!("{l_skip}:"));
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RLF 0x{t_lo:02X}, F"));
                self.emit(format!("    RLF 0x{t_hi:02X}, F")); // t <<= 1
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RRF 0x{bk:02X}, F")); // bk >>= 1
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                self.store_retval(r_lo, 2);
                self.emit("    RETURN".to_string());
            }
            // 16x16 -> 32 shift-add, 16 iterations: t = a (32-bit, shifted
            // left), for each set bit of bk, r += t across all 4 bytes with
            // the incfsz carry idiom. Store the low 16 bits (the i16 result).
            "__mul_u16" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, a + 1, b, b + 1, scr, scr + 10], name);
                let (bk_lo, bk_hi, cnt) = (scr, scr + 1, scr + 2);
                let (r0, r1, r2, r3) = (scr + 3, scr + 4, scr + 5, scr + 6);
                let (t0, t1, t2, t3) = (scr + 7, scr + 8, scr + 9, scr + 10);
                let l_loop = self.fresh_label();
                let l_skip = self.fresh_label();
                for r in [r0, r1, r2, r3] {
                    self.emit(format!("    CLRF 0x{r:02X}"));
                }
                for t in [t0, t1, t2, t3] {
                    self.emit(format!("    CLRF 0x{t:02X}"));
                }
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    MOVWF 0x{t0:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", a + 1));
                self.emit(format!("    MOVWF 0x{t1:02X}")); // t = a (32-bit, low 16)
                self.emit(format!("    MOVF 0x{b:02X}, W"));
                self.emit(format!("    MOVWF 0x{bk_lo:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", b + 1));
                self.emit(format!("    MOVWF 0x{bk_hi:02X}")); // bk = b
                self.emit("    MOVLW 0x10".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}")); // cnt = 16
                self.emit(format!("{l_loop}:"));
                self.emit(format!("    BTFSS 0x{bk_lo:02X}, 0")); // test multiplier LSB
                self.emit(format!("    GOTO {l_skip}"));
                self.emit(format!("    MOVF 0x{t0:02X}, W"));
                self.emit(format!("    ADDWF 0x{r0:02X}, F"));
                self.emit(format!("    MOVF 0x{t1:02X}, W"));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{t1:02X}, W"));
                self.emit(format!("    ADDWF 0x{r1:02X}, F"));
                self.emit(format!("    MOVF 0x{t2:02X}, W"));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{t2:02X}, W"));
                self.emit(format!("    ADDWF 0x{r2:02X}, F"));
                self.emit(format!("    MOVF 0x{t3:02X}, W"));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{t3:02X}, W"));
                self.emit(format!("    ADDWF 0x{r3:02X}, F"));
                self.emit(format!("{l_skip}:"));
                self.emit("    BCF STATUS, 0".to_string());
                for t in [t0, t1, t2, t3] {
                    self.emit(format!("    RLF 0x{t:02X}, F")); // t <<= 1
                }
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RRF 0x{bk_hi:02X}, F"));
                self.emit(format!("    RRF 0x{bk_lo:02X}, F")); // bk >>= 1
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                self.store_retval(r0, 2);
                self.emit("    RETURN".to_string());
            }
            // 8/8 restoring division (8 iterations): num <<= 1 (C = old
            // MSB); rem = (rem << 1) | C; if rem >= den set the quotient bit
            // else restore (add den back). rem is 2 bytes: the 8-bit rem
            // shift can carry. Borrow idiom: den_hi is implicitly 0, so the
            // fold is `movlw 0; btfss C; addlw 1; subwf rem_hi`.
            "__udiv_u8" | "__urem_u8" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, den, scr, scr + 3], name);
                let (rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2);
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                self.emit(format!("    CLRF 0x{rem_lo:02X}"));
                self.emit(format!("    CLRF 0x{rem_hi:02X}"));
                self.emit("    MOVLW 0x08".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}"));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RLF 0x{num:02X}, F"));
                self.emit(format!("    RLF 0x{rem_lo:02X}, F"));
                self.emit(format!("    RLF 0x{rem_hi:02X}, F"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    SUBWF 0x{rem_lo:02X}, F"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string()); // W = borrow
                self.emit(format!("    SUBWF 0x{rem_hi:02X}, F")); // C = (rem >= den)
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit(format!("    BSF 0x{num:02X}, 0"));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    ADDWF 0x{rem_lo:02X}, F"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string()); // W = carry
                self.emit(format!("    ADDWF 0x{rem_hi:02X}, F"));
                self.emit(format!("{l_next}:"));
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__udiv_u8" {
                    self.store_retval(num, 1);
                } else {
                    self.store_retval(rem_lo, 1);
                }
                self.emit("    RETURN".to_string());
            }
            // 16/16 restoring division (16 iterations), the borrow idiom
            // `movf den_hi,w; btfss C; incfsz den_hi,w; subwf rem_hi,f`.
            "__udiv_u16" | "__urem_u16" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, num + 1, den, den + 1, scr, scr + 6], name);
                let (rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2);
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                self.emit(format!("    CLRF 0x{rem_lo:02X}"));
                self.emit(format!("    CLRF 0x{rem_hi:02X}"));
                self.emit("    MOVLW 0x10".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}"));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RLF 0x{num:02X}, F"));
                self.emit(format!("    RLF 0x{:02X}, F", num + 1));
                self.emit(format!("    RLF 0x{rem_lo:02X}, F"));
                self.emit(format!("    RLF 0x{rem_hi:02X}, F"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    SUBWF 0x{rem_lo:02X}, F"));
                self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1)); // den_hi + borrow
                self.emit(format!("    SUBWF 0x{rem_hi:02X}, F")); // C = (rem >= den)
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit(format!("    BSF 0x{num:02X}, 0"));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    ADDWF 0x{rem_lo:02X}, F"));
                self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1)); // den_hi + carry
                self.emit(format!("    ADDWF 0x{rem_hi:02X}, F"));
                self.emit(format!("{l_next}:"));
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__udiv_u16" {
                    self.store_retval(num, 2);
                } else {
                    self.store_retval(rem_lo, 2);
                }
                self.emit("    RETURN".to_string());
            }
            // Signed 8-bit wrappers: abs both operands in place in the param
            // slots (unsigned abs, INT_MIN safe), run the unsigned divmod,
            // negate the quotient if the signs differed (bit0) / the
            // remainder if the dividend was negative (bit1).
            "__sdiv_i8" | "__srem_i8" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, den, scr, scr + 4], name);
                let (flags, rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2, scr + 3);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit(format!("    CLRF 0x{flags:02X}"));
                self.emit(format!("    BTFSS 0x{num:02X}, 7"));
                self.emit(format!("    GOTO {l_den}"));
                self.emit(format!("    BSF 0x{flags:02X}, 1")); // remainder sign follows dividend
                self.emit(format!("    BSF 0x{flags:02X}, 0")); // quotient negate: num<0
                self.emit(format!("    COMF 0x{num:02X}, F"));
                self.emit(format!("    INCF 0x{num:02X}, F")); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit(format!("    BTFSS 0x{den:02X}, 7"));
                self.emit(format!("    GOTO {l_go}"));
                self.emit(format!("    COMF 0x{den:02X}, F"));
                self.emit(format!("    INCF 0x{den:02X}, F")); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit(format!("    XORWF 0x{flags:02X}, F")); // bit0 ^= den<0: neg_q = num<0 XOR den<0
                self.emit(format!("{l_go}:"));
                self.emit(format!("    CLRF 0x{rem_lo:02X}"));
                self.emit(format!("    CLRF 0x{rem_hi:02X}"));
                self.emit("    MOVLW 0x08".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}"));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RLF 0x{num:02X}, F"));
                self.emit(format!("    RLF 0x{rem_lo:02X}, F"));
                self.emit(format!("    RLF 0x{rem_hi:02X}, F"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    SUBWF 0x{rem_lo:02X}, F"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    SUBWF 0x{rem_hi:02X}, F"));
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit(format!("    BSF 0x{num:02X}, 0"));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    ADDWF 0x{rem_lo:02X}, F"));
                self.emit("    MOVLW 0x00".to_string());
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    ADDWF 0x{rem_hi:02X}, F"));
                self.emit(format!("{l_next}:"));
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__sdiv_i8" {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 0"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.emit(format!("    COMF 0x{num:02X}, F"));
                    self.emit(format!("    INCF 0x{num:02X}, F"));
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 1);
                } else {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 1"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.emit(format!("    COMF 0x{rem_lo:02X}, F"));
                    self.emit(format!("    INCF 0x{rem_lo:02X}, F"));
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem_lo, 1);
                }
                self.emit("    RETURN".to_string());
            }
            // Signed 16-bit wrappers: same structure, 16-bit abs/negate and
            // the 16-bit divmod with the incfsz borrow idiom.
            "__sdiv_i16" | "__srem_i16" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, num + 1, den, den + 1, scr, scr + 6], name);
                let (flags, rem_lo, rem_hi, cnt) = (scr, scr + 1, scr + 2, scr + 3);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_loop = self.fresh_label();
                let l_restore = self.fresh_label();
                let l_next = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit(format!("    CLRF 0x{flags:02X}"));
                self.emit(format!("    BTFSS 0x{:02X}, 7", num + 1));
                self.emit(format!("    GOTO {l_den}"));
                self.emit(format!("    BSF 0x{flags:02X}, 1")); // remainder sign follows dividend
                self.emit(format!("    BSF 0x{flags:02X}, 0")); // quotient negate: num<0
                self.neg16_in_place(num); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit(format!("    BTFSS 0x{:02X}, 7", den + 1));
                self.emit(format!("    GOTO {l_go}"));
                self.neg16_in_place(den); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit(format!("    XORWF 0x{flags:02X}, F")); // bit0 ^= den<0: neg_q = num<0 XOR den<0
                self.emit(format!("{l_go}:"));
                self.emit(format!("    CLRF 0x{rem_lo:02X}"));
                self.emit(format!("    CLRF 0x{rem_hi:02X}"));
                self.emit("    MOVLW 0x10".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}"));
                self.emit(format!("{l_loop}:"));
                self.emit("    BCF STATUS, 0".to_string());
                self.emit(format!("    RLF 0x{num:02X}, F"));
                self.emit(format!("    RLF 0x{:02X}, F", num + 1));
                self.emit(format!("    RLF 0x{rem_lo:02X}, F"));
                self.emit(format!("    RLF 0x{rem_hi:02X}, F"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    SUBWF 0x{rem_lo:02X}, F"));
                self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1));
                self.emit(format!("    SUBWF 0x{rem_hi:02X}, F"));
                self.emit("    BTFSS STATUS, 0".to_string());
                self.emit(format!("    GOTO {l_restore}"));
                self.emit(format!("    BSF 0x{num:02X}, 0"));
                self.emit(format!("    GOTO {l_next}"));
                self.emit(format!("{l_restore}:"));
                self.emit(format!("    MOVF 0x{den:02X}, W"));
                self.emit(format!("    ADDWF 0x{rem_lo:02X}, F"));
                self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1));
                self.emit(format!("    ADDWF 0x{rem_hi:02X}, F"));
                self.emit(format!("{l_next}:"));
                self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
                self.emit(format!("    GOTO {l_loop}"));
                if recipe == "__sdiv_i16" {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 0"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg16_in_place(num); // -quotient
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 2);
                } else {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 1"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg16_in_place(rem_lo); // -remainder
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem_lo, 2);
                }
                self.emit("    RETURN".to_string());
            }
            // 32x32 -> 32 shift-add (AN526), 32 iterations: t = a (4 bytes,
            // shifted left one bit per iteration: the shifted-out high
            // bits are DISCARDED, so i32 mul wraps mod 2^32); for each set
            // bit of the multiplier, r += t across all 4 bytes with the
            // incfsz carry idiom. bk is 2 bytes: the low 16 multiplier bits
            // first, then reloaded from b's high half for the second 16
            // iterations (the b param slot is untouched). Store the low 32
            // bits (the i32 result).
            "__mul_u32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, a + 3, b, b + 3, scr, scr + 10], name);
                let (bk_lo, bk_hi, cnt) = (scr, scr + 1, scr + 2);
                let r = [scr + 3, scr + 4, scr + 5, scr + 6];
                let t = [scr + 7, scr + 8, scr + 9, scr + 10];
                for i in 0..4 {
                    self.emit(format!("    CLRF 0x{:02X}", r[i]));
                    self.emit(format!("    CLRF 0x{:02X}", t[i]));
                }
                for i in 0..4u16 {
                    self.emit(format!("    MOVF 0x{:02X}, W", a + i));
                    self.emit(format!("    MOVWF 0x{:02X}", t[usize::from(i)]));
                    // t = a (32-bit)
                }
                self.emit(format!("    MOVF 0x{:02X}, W", b));
                self.emit(format!("    MOVWF 0x{bk_lo:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", b + 1));
                self.emit(format!("    MOVWF 0x{bk_hi:02X}")); // bk = b low 16
                self.emit("    MOVLW 0x10".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}")); // cnt = 16
                let l_loop1 = self.fresh_label();
                let l_skip1 = self.fresh_label();
                self.emit_mul32_loop(l_loop1, l_skip1, bk_lo, bk_hi, cnt, r, t);
                // reload bk from b's high half for the second 16 iterations
                self.emit(format!("    MOVF 0x{:02X}, W", b + 2));
                self.emit(format!("    MOVWF 0x{bk_lo:02X}"));
                self.emit(format!("    MOVF 0x{:02X}, W", b + 3));
                self.emit(format!("    MOVWF 0x{bk_hi:02X}"));
                self.emit("    MOVLW 0x10".to_string());
                self.emit(format!("    MOVWF 0x{cnt:02X}"));
                let l_loop2 = self.fresh_label();
                let l_skip2 = self.fresh_label();
                self.emit_mul32_loop(l_loop2, l_skip2, bk_lo, bk_hi, cnt, r, t);
                self.store_retval(scr + 3, 4);
                self.emit("    RETURN".to_string());
            }
            // 32/32 restoring division (32 iterations): num <<= 1 (C = old
            // MSB, brought down into rem's LSB); rem = (rem << 1) | C; if
            // rem >= den set the quotient bit else restore (add den back).
            // The full-width 4-byte remainder never carries out (rem <=
            // 2^k - 1 before the k-th shift), so the 4-byte borrow chain
            // with the INCFSZ wrap-correct folds is exact. den is copied
            // into __scr@4-7 (the divmod reads it repeatedly).
            "__udiv_u32" | "__urem_u32" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, num + 3, den, den + 3, scr, scr + 9], name);
                for i in 0..4 {
                    self.emit(format!("    MOVF 0x{:02X}, W", den + i));
                    self.emit(format!("    MOVWF 0x{:02X}", scr + 4 + i)); // den copy
                }
                self.emit_divmod32(num, scr);
                if recipe == "__udiv_u32" {
                    self.store_retval(num, 4);
                } else {
                    self.store_retval(scr, 4);
                }
                self.emit("    RETURN".to_string());
            }
            // Signed 32-bit wrappers: abs both operands in place in the
            // param slots (unsigned abs, INT_MIN safe: |INT_MIN| wraps to
            // itself, deterministic), run the unsigned divmod, negate the
            // quotient if the signs differed (bit0 = num<0 XOR den<0) / the
            // remainder if the dividend was negative (bit1).
            "__sdiv_i32" | "__srem_i32" => {
                let num = self.slot_addr(name, "num").direct();
                let den = self.slot_addr(name, "den").direct();
                self.assert_bank0(&[num, num + 3, den, den + 3, scr, scr + 11], name);
                let (rem, den_s, flags) = (scr, scr + 4, scr + 10);
                let l_den = self.fresh_label();
                let l_go = self.fresh_label();
                let l_store = self.fresh_label();
                self.emit(format!("    CLRF 0x{flags:02X}"));
                self.emit(format!("    BTFSS 0x{:02X}, 7", num + 3));
                self.emit(format!("    GOTO {l_den}"));
                self.emit(format!("    BSF 0x{flags:02X}, 1")); // remainder sign follows dividend
                self.emit(format!("    BSF 0x{flags:02X}, 0")); // quotient negate: num<0
                self.neg32_in_place(num); // num = |num|
                self.emit(format!("{l_den}:"));
                self.emit(format!("    BTFSS 0x{:02X}, 7", den + 3));
                self.emit(format!("    GOTO {l_go}"));
                self.neg32_in_place(den); // den = |den|
                self.emit("    MOVLW 0x01".to_string());
                self.emit(format!("    XORWF 0x{flags:02X}, F")); // bit0 ^= den<0: neg_q = num<0 XOR den<0
                self.emit(format!("{l_go}:"));
                for i in 0..4 {
                    self.emit(format!("    MOVF 0x{:02X}, W", den + i));
                    self.emit(format!("    MOVWF 0x{:02X}", den_s + i)); // |den| copy
                }
                self.emit_divmod32(num, scr);
                if recipe == "__sdiv_i32" {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 0"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg32_in_place(num); // -quotient
                    self.emit(format!("{l_store}:"));
                    self.store_retval(num, 4);
                } else {
                    self.emit(format!("    BTFSS 0x{flags:02X}, 1"));
                    self.emit(format!("    GOTO {l_store}"));
                    self.neg32_in_place(rem); // -remainder
                    self.emit(format!("{l_store}:"));
                    self.store_retval(rem, 4);
                }
                self.emit("    RETURN".to_string());
            }
            // The nine soft-float routines (Milestone 15): hand-written
            // IEEE754 recipes, round-to-nearest-even. The float format: 4
            // bytes LE: b0 = mantissa LSB, b1, b2 = mantissa MSB + the
            // exponent's LSB (bit 7 of b2), b3 = sign | exponent[7:1]; the
            // 24-bit mantissa = (b2 & 0x7F) << 16 | b1 << 8 | b0, plus the
            // implicit 0x800000 when the 8-bit biased exponent ((b3 & 0x7F)
            // << 1 | (b2 >> 7)) is nonzero. Args arrive in the routine's
            // param slots; the result goes to the fixed retval region
            // (0x71-0x74); working state lives in `__scr` at the Task-2
            // contract offsets. All slots stay inside one GPR bank (any
            // bank, issue #6), the loops are skip-sensitive.
            "__add_f32" | "__sub_f32" => {
                let pa = self.slot_addr(name, "a").direct();
                let pb = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[pa, pa + 3, pb, pb + 3, scr, scr + 13], name);
                // __sub_f32 = flip b's sign bit, then the add path.
                self.emit_f32_extract(pa, scr, scr + 1, scr + 2, false);
                self.emit_f32_extract(pb, scr + 5, scr + 6, scr + 7, recipe == "__sub_f32");
                self.emit_f32_add_body(scr);
            }
            "__mul_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, a + 3, b, b + 3, scr, scr + 13], name);
                self.emit_f32_mul_body(a, b, scr);
            }
            "__div_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, a + 3, b, b + 3, scr, scr + 11], name);
                self.emit_f32_div_body(a, b, scr);
            }
            "__uitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                self.assert_bank0(&[val, val + 3, scr, scr + 7], name);
                self.emit_f32_uitofp_body(val, scr, None);
            }
            "__sitofp_f32" => {
                let val = self.slot_addr(name, "val").direct();
                self.assert_bank0(&[val, val + 3, scr, scr + 7], name);
                // Save the sign, abs in place (unsigned abs, INT_MIN wraps
                // to itself, deterministic), then the uitofp path.
                let sign = scr + 5;
                let l_pos = self.fresh_label();
                self.emit(format!("    MOVF 0x{:02X}, W", val + 3));
                self.emit("    ANDLW 0x80".to_string());
                self.emit(format!("    MOVWF 0x{sign:02X}"));
                self.emit(format!("    BTFSS 0x{:02X}, 7", val + 3));
                self.emit(format!("    GOTO {l_pos}"));
                self.neg32_in_place(val);
                self.emit(format!("{l_pos}:"));
                self.emit_f32_uitofp_body(val, scr, Some(sign));
            }
            "__fptoui_f32" | "__fptosi_f32" => {
                let val = self.slot_addr(name, "val").direct();
                self.assert_bank0(&[val, val + 3, scr, scr + 7], name);
                self.emit_f32_fptoi_body(val, scr, recipe == "__fptosi_f32");
            }
            "__cmp_f32" => {
                let a = self.slot_addr(name, "a").direct();
                let b = self.slot_addr(name, "b").direct();
                self.assert_bank0(&[a, a + 3, b, b + 3, scr, scr + 5], name);
                self.emit_f32_cmp_body(a, b, scr);
            }
            other => panic!("isel: no recipe for runtime routine @{other}"),
        }
    }

    /// The recipe body for the nine variable-count shift routines (i8/i16/
    /// i32). The count arrives UNMASKED (a full i8/i16/i32, clang emits
    /// it raw); LLVM says counts >= width are poison, so masking to
    /// width-1 keeps the loop bounded (<= 7/15/31 iterations) and yields
    /// the defined-range result: deterministic, documented, never a hang.
    /// The value shifts **in place in the `val` param slot** (the caller's
    /// copy); the masked count runs the loop from `__scr::cnt@0` (the
    /// layout-contract offset). ashr sets C from the sign bit before each
    /// rrf so the sign fills every vacated bit. All slots stay inside one
    /// GPR bank (any bank, issue #6), the loops are skip-sensitive.
    fn emit_shift_body(&mut self, bytes: u16, op: BinOp, scr: u16) {
        let name = self.cur_func;
        let val = self.slot_addr(name, "val").direct();
        let cnt = self.slot_addr(name, "cnt").direct();
        let hi = val + bytes - 1;
        self.assert_bank0(&[val, hi, cnt, cnt + bytes - 1, scr, scr + 1], name);
        let mask: u8 = match bytes {
            1 => 0x07,
            2 => 0x0F,
            4 => 0x1F,
            _ => unreachable!("isel: shift body width"),
        }; // width - 1
        self.emit(format!("    MOVF 0x{cnt:02X}, W"));
        self.emit(format!("    ANDLW 0x{mask:02X}")); // count & (width-1)
        self.emit(format!("    MOVWF 0x{scr:02X}")); // __scr::cnt@0 = masked count
        if bytes == 2 {
            // __scr::cnt@1 (the high byte of the masked 2-byte cnt slot)
            // stays 0: the masked count is < 16, so the DECFSZ loop counter
            // lives entirely in the low byte. Clear it once so a stale high
            // byte from an earlier call can't be misread as part of the count.
            self.emit(format!("    CLRF 0x{:02X}", scr + 1));
        }
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        // count == 0 shifts nothing: skip the loop entirely (a bare
        // DECFSZ-at-bottom loop would run once on a zero counter).
        self.emit(format!("    MOVF 0x{scr:02X}, F")); // Z = (cnt == 0)
        self.emit("    BTFSC STATUS, 2".to_string()); // skip the GOTO when cnt != 0
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_loop}:"));
        match op {
            BinOp::Shl => {
                self.emit("    BCF STATUS, 0".to_string());
                for i in 0..bytes {
                    self.emit(format!("    RLF 0x{:02X}, F", val + i));
                }
            }
            BinOp::LShr => {
                self.emit("    BCF STATUS, 0".to_string());
                for i in (0..bytes).rev() {
                    self.emit(format!("    RRF 0x{:02X}, F", val + i));
                }
            }
            BinOp::AShr => {
                self.emit(format!("    BTFSC 0x{hi:02X}, 7"));
                self.emit("    BSF STATUS, 0".to_string());
                self.emit(format!("    BTFSS 0x{hi:02X}, 7"));
                self.emit("    BCF STATUS, 0".to_string());
                for i in (0..bytes).rev() {
                    self.emit(format!("    RRF 0x{:02X}, F", val + i));
                }
            }
            _ => unreachable!(),
        }
        self.emit(format!("    DECFSZ 0x{scr:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_done}:"));
        self.store_retval(val, bytes as u8);
        self.emit("    RETURN".to_string());
    }

    // -----------------------------------------------------------------------
    // The soft-float routine recipes (Milestone 15, Task 3).
    // -----------------------------------------------------------------------
    //
    // IEEE754 single (f32) = 4 bytes LE: b0 = mantissa LSB, b1, b2 =
    // mantissa MSB + the exponent's LSB (bit 7 of b2), b3 = sign |
    // exponent[7:1]. The 24-bit mantissa = (b2 & 0x7F) << 16 | b1 << 8 | b0,
    // plus the implicit 0x800000 when the 8-bit biased exponent ((b3 & 0x7F)
    // << 1 | (b2 >> 7)) is nonzero. Round-to-nearest-even: round up iff
    // guard && (sticky || mantissa LSB); on a rounding carry the mantissa
    // renormalizes (0x800000, exp+1). The retval region is 0x71-0x74.

    /// Swap two bytes via the XOR trick (no scratch needed). Each XORWF
    /// consumes its operand from W, so W must be reloaded between the steps
    /// (a stale W from the first load would zero the first byte instead of
    /// swapping).
    fn emit_xor_swap(&mut self, x: u16, y: u16) {
        self.emit(format!("    MOVF 0x{y:02X}, W"));
        self.emit(format!("    XORWF 0x{x:02X}, F"));
        self.emit(format!("    MOVF 0x{x:02X}, W"));
        self.emit(format!("    XORWF 0x{y:02X}, F"));
        self.emit(format!("    MOVF 0x{y:02X}, W"));
        self.emit(format!("    XORWF 0x{x:02X}, F"));
    }

    /// Extract an f32 param slot into `sign` (bit 7), the full 8-bit biased
    /// exponent into `exp`, and the 24-bit mantissa with the implicit bit
    /// into `mant`..`mant+2`. A zero exponent means the value is +/-0 (or a
    /// denormal, treated as 0): the mantissa clears. `flip` XORs the sign
    /// (__sub_f32). A DENORMAL (exp 0, nonzero fraction) keeps its fraction
    /// WITHOUT the implicit bit (issue #11): the old code cleared the
    /// whole mantissa, so denormal + denormal summed to 0 instead of the
    /// denormal sum.
    fn emit_f32_extract(&mut self, slot: u16, sign: u16, exp: u16, mant: u16, flip: bool) {
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 3));
        self.emit("    ANDLW 0x80".to_string());
        if flip {
            self.emit("    XORLW 0x80".to_string());
        }
        self.emit(format!("    MOVWF 0x{sign:02X}"));
        // exp = (b3 & 0x7F) << 1 | (b2 >> 7)
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{exp:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{exp:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", slot + 2));
        self.emit(format!("    BSF 0x{exp:02X}, 0"));
        // mant = b0, b1, (b2 & 0x7F) | 0x80 (the implicit bit, except for
        // a denormal, exp 0, which has no implicit bit).
        self.emit(format!("    MOVF 0x{:02X}, W", slot));
        self.emit(format!("    MOVWF 0x{:02X}", mant));
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 1));
        self.emit(format!("    MOVWF 0x{:02X}", mant + 1));
        self.emit_f32_mant_hi(slot);
        self.emit(format!("    MOVWF 0x{:02X}", mant + 2));
        // A denormal (exp 0, fraction nonzero) aligns at the exp-1 scale:
        // its value is frac x 2^-149 = frac x 2^(1-127-23), so the
        // alignment treats it as exp 1 with the raw fraction (no implicit
        // bit). ±0 (exp 0, fraction 0) stays exp 0.
        let l_den_done = self.fresh_label();
        self.emit(format!("    MOVF 0x{exp:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", mant));
        self.emit(format!("    IORWF 0x{:02X}, W", mant + 1));
        self.emit(format!("    IORWF 0x{:02X}, W", mant + 2));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{exp:02X}"));
        self.emit(format!("{l_den_done}:"));
    }

    /// RNE round-up: mantissa += 1 across the 3 bytes; on a 24-bit carry the
    /// mantissa renormalizes to 0x800000 with `e += 1`. The carry can only
    /// fire on a full mantissa (m2 == 0xFF, top bit set, a normal), so at
    /// e == 1 it renormalizes into e == 2 (the smallest-normal binade),
    /// which is correct: 0xFFFFFF x 2^-149 + half-ulp rounds to 2^-125.
    fn emit_f32_round_up(&mut self, m0: u16, m1: u16, m2: u16, e: u16) {
        let l_renorm = self.fresh_label();
        let l_done = self.fresh_label();
        self.emit(format!("    INCF 0x{m0:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{m1:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{m2:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_renorm}:"));
        self.emit(format!("    MOVLW 0x80"));
        self.emit(format!("    MOVWF 0x{m2:02X}"));
        self.emit(format!("    CLRF 0x{m1:02X}"));
        self.emit(format!("    CLRF 0x{m0:02X}"));
        self.emit(format!("    MOVLW 0x01"));
        self.emit(format!("    ADDWF 0x{e:02X}, F"));
        self.emit(format!("{l_done}:"));
    }

    /// Assemble the result into the fixed retval region (0x71-0x74): b0 =
    /// m0, b1 = m1, b2 = (m2 & 0x7F) | (e & 1) << 7, b3 = (e >> 1) | sign.
    fn emit_f32_assemble(&mut self, sign: u16, e: u16, m0: u16, m1: u16, m2: u16) {
        let r = self.retval_lo;
        self.emit(format!("    MOVF 0x{m0:02X}, W"));
        self.emit(format!("    MOVWF 0x{:02X}", r));
        self.emit(format!("    MOVF 0x{m1:02X}, W"));
        self.emit(format!("    MOVWF 0x{:02X}", r + 1));
        self.emit(format!("    MOVLW 0x7F"));
        self.emit(format!("    ANDWF 0x{m2:02X}, W"));
        self.emit(format!("    MOVWF 0x{:02X}", r + 2));
        self.emit(format!("    BTFSC 0x{e:02X}, 0"));
        self.emit(format!("    BSF 0x{:02X}, 7", r + 2));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit(format!("    MOVWF 0x{:02X}", r + 3));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", r + 3));
        self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
        self.emit(format!("    BSF 0x{:02X}, 7", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// Load `slot+2`'s fraction into W and OR the implicit bit unless the
    /// operand is a denormal (full 8-bit exponent 0, no implicit bit,
    /// issue #11). The caller stores W into the mantissa's high byte.
    fn emit_f32_mant_hi(&mut self, slot: u16) {
        let l_imp = self.fresh_label();
        let l_done = self.fresh_label();
        // denormal check: exp 0 = (b3 & 0x7F) == 0 && !(b2 bit 7)
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_imp}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", slot + 2));
        self.emit(format!("    GOTO {l_imp}"));
        // exp 0 (denormal): fraction only, no implicit bit.
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    GOTO {l_done}"));
        self.emit(format!("{l_imp}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("{l_done}:"));
    }

    /// Emit the fixed quiet-NaN result (0x7FC00000 | sign) and RETURN.
    /// The sign is the caller's computed result sign (IEEE leaves the NaN
    /// sign unspecified; the class is what matters).
    fn emit_f32_nan(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit(format!("    CLRF 0x{:02X}", r));
        self.emit(format!("    CLRF 0x{:02X}", r + 1));
        self.emit("    MOVLW 0xC0".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", r + 2));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", r + 3));
        self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
        self.emit(format!("    BSF 0x{:02X}, 7", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// Emit the fixed infinity result (0x7F800000 | sign) and RETURN.
    fn emit_f32_inf(&mut self, sign: u16) {
        let r = self.retval_lo;
        self.emit(format!("    CLRF 0x{:02X}", r));
        self.emit(format!("    CLRF 0x{:02X}", r + 1));
        self.emit("    MOVLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", r + 2));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", r + 3));
        self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
        self.emit(format!("    BSF 0x{:02X}, 7", r + 3));
        self.emit("    RETURN".to_string());
    }

    /// The __add_f32 / __sub_f32 body (both operands already extracted at the
    /// contract offsets: sa@0, ea@1, ma@2-4, sb@5, eb@6, mb@7-9, stick@10
    /// (bit 0 = round, bit 1 = the OR of the bits below the 24-bit fraction
    /// window), cnt@11, ta1@12, ta2@13; `ta0` reuses the dead `eb` slot).
    /// Extract, align the smaller exponent's mantissa right by the
    /// difference (clamped to 31), then add or subtract at the larger
    /// exponent's scale (the result exponent register becomes eb).
    ///
    /// The alignment builds the lost fraction EXACTLY: each shifted-out bit
    /// is inserted at the top of a 24-bit window `ta` (an RRF chain, the
    /// last bit out, the round bit, lands at ta2 bit 7), and the bits that
    /// overflow the window's bottom are OR'd into `stick` bit 1. The value
    /// is the 6-byte integer `ma:ta` at the result scale.
    ///
    /// The ADD path is exact: the sum never normalizes (a carry shifts
    /// right once, promoting the old round bit into the sticky), so RNE
    /// reads round = ta2 bit 7 and sticky = OR(ta below the top bit) |
    /// stick bit 1.
    ///
    /// The SUBTRACT path is exact in the same 6-byte value: the result is
    /// |a| - |b| = (ma - mb) - frac, computed as a fractional borrow
    /// (ma -= 1 and ta = 2^24 - ta, the deep OR folded into ta's LSB)
    /// followed by the plain 3-byte subtract. The 6-byte value then
    /// normalizes with a 6-byte left shift: the fraction's bits move into
    /// the mantissa one at a time, and RNE reads the guard from ta2 bit 7,
    /// the sticky from ta's low bits | stick bit 1. A single wrong RNE bit
    /// is impossible. (The earlier two-bit round/sticky model was inexact
    /// once the fraction's tail drained to a power of two: the M15 float
    /// differential found 6/2000 SIM sub mismatches, e.g. 1.0 - 0x3EFFFFFF
    /// over-rounded to 0x3F000001 instead of the RNE 0x3F000000.)
    fn emit_f32_add_body(&mut self, scr: u16) {
        let (sa, ea) = (scr, scr + 1);
        let (ma0, ma1, ma2) = (scr + 2, scr + 3, scr + 4);
        let (sb, eb) = (scr + 5, scr + 6);
        let (mb0, mb1, mb2) = (scr + 7, scr + 8, scr + 9);
        let (stick, cnt) = (scr + 10, scr + 11);
        let (ta0, ta1, ta2) = (scr + 6, scr + 12, scr + 13); // ta0 reuses eb
        let l_ma_nz = self.fresh_label();
        let l_copy_b = self.fresh_label();
        let l_zero = self.fresh_label();
        let l_ma_nz2 = self.fresh_label();
        let l_no_swap = self.fresh_label();
        let l_no_clamp = self.fresh_label();
        let l_align_loop = self.fresh_label();
        let l_align_done = self.fresh_label();
        let l_sub = self.fresh_label();
        let l_add_carry = self.fresh_label();
        let l_cmp_b1 = self.fresh_label();
        let l_cmp_b0 = self.fresh_label();
        let l_cmp_frac = self.fresh_label();
        let l_sub_equal_frac = self.fresh_label();
        let l_sub_swap = self.fresh_label();
        let l_sub_done = self.fresh_label();
        let l_sub_no_frac = self.fresh_label();
        let l_sub_borrow_done = self.fresh_label();
        let l_normalize = self.fresh_label();
        let l_sub_guard = self.fresh_label();
        let l_round_step = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_zs_clear = self.fresh_label();
        let l_zs_done = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_inf = self.fresh_label();
        let l_a_not_inf = self.fresh_label();
        let l_b_not_inf = self.fresh_label();
        let l_a_nan_done = self.fresh_label();
        let l_b_nan_done = self.fresh_label();
        let l_inf_done = self.fresh_label();
        // ---- IEEE specials (issue #11): NaN and infinity operands ----
        // NaN a: exp 0xFF (ea == 0xFF) && FRACTION nonzero (the extracted
        // mantissa carries the implicit bit, so inf's 0x800000 must not
        // read as a NaN, test ma2 & 0x7F | ma1 | ma0). `cnt` is dead at
        // this point (the alignment sets it later).
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_nan_done}"));
        self.emit(format!("    MOVF 0x{ma0:02X}, W"));
        self.emit(format!("    IORWF 0x{ma1:02X}, W"));
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("    MOVF 0x{ma2:02X}, W"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nan_done}:"));
        // NaN b
        self.emit(format!("    MOVF 0x{eb:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_nan_done}"));
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    IORWF 0x{mb1:02X}, W"));
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("    MOVF 0x{mb2:02X}, W"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_nan_done}:"));
        // inf a? (exp 0xFF, mantissa 0, the NaN checks above already
        // routed mantissa-nonzero exp-0xFF operands to l_nan).
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_not_inf}"));
        // a is inf: b inf? both inf -> same sign inf, opposite NaN.
        self.emit(format!("    MOVF 0x{eb:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_not_inf}"));
        self.emit(format!("    MOVF 0x{sa:02X}, W"));
        self.emit(format!("    XORWF 0x{sb:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_not_inf}:"));
        // a inf, b finite: result inf (a's sign).
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_not_inf}:"));
        // a finite: b inf? result inf (b's sign).
        self.emit(format!("    MOVF 0x{eb:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf_done}"));
        self.emit(format!("    MOVF 0x{sb:02X}, W"));
        self.emit(format!("    MOVWF 0x{sa:02X}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sa);
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sa);
        self.emit(format!("{l_inf_done}:"));
        // ---- zero operand handling ----
        self.emit(format!("    MOVF 0x{ma0:02X}, W"));
        self.emit(format!("    IORWF 0x{ma1:02X}, W"));
        self.emit(format!("    IORWF 0x{ma2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_ma_nz}"));
        // ma == 0: mb == 0 -> +/-0, else the result is b exactly.
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    IORWF 0x{mb1:02X}, W"));
        self.emit(format!("    IORWF 0x{mb2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_copy_b}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_copy_b}:"));
        for (dst, src) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit(format!("    MOVF 0x{src:02X}, W"));
            self.emit(format!("    MOVWF 0x{dst:02X}"));
        }
        self.emit(format!("    CLRF 0x{stick:02X}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz}:"));
        // mb == 0 (ma != 0): the result is a exactly.
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    IORWF 0x{mb1:02X}, W"));
        self.emit(format!("    IORWF 0x{mb2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_ma_nz2}"));
        self.emit(format!("    CLRF 0x{stick:02X}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_ma_nz2}:"));
        self.emit(format!("    CLRF 0x{stick:02X}"));
        self.emit(format!("    CLRF 0x{ta1:02X}"));
        self.emit(format!("    CLRF 0x{ta2:02X}"));
        // ---- swap so that a is the smaller-exponent operand ----
        self.emit(format!("    MOVF 0x{eb:02X}, W"));
        self.emit(format!("    SUBWF 0x{ea:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string()); // C=1 (ea >= eb) -> swap
        self.emit(format!("    GOTO {l_no_swap}"));
        for (x, y) in [(sa, sb), (ea, eb), (ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit(format!("{l_no_swap}:"));
        // ---- alignment: diff = eb - ea (a is the smaller exponent),
        //      clamped to 31, shift ma right. The result exponent is the
        //      LARGER one (eb): the sum/difference is at its scale, so the
        //      result-exp register becomes eb. ----
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit(format!("    SUBWF 0x{eb:02X}, W"));
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("    MOVF 0x{eb:02X}, W"));
        self.emit(format!("    MOVWF 0x{ea:02X}"));
        self.emit(format!("    CLRF 0x{ta0:02X}")); // eb is dead; ta0 = 0
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    SUBWF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_no_clamp}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_no_clamp}:"));
        self.emit(format!("    MOVF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("    GOTO {l_align_done}"));
        self.emit(format!("{l_align_loop}:"));
        // ma >>= 1; the shifted-out bit enters the TOP of the 24-bit
        // fraction window ta (the last bit out = the round, at ta2 bit 7);
        // bits pushed out the window's bottom accumulate in stick bit 1.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{ma2:02X}, F"));
        self.emit(format!("    RRF 0x{ma1:02X}, F"));
        self.emit(format!("    RRF 0x{ma0:02X}, F"));
        self.emit(format!("    RRF 0x{ta2:02X}, F"));
        self.emit(format!("    RRF 0x{ta1:02X}, F"));
        self.emit(format!("    RRF 0x{ta0:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{stick:02X}, 1"));
        self.emit(format!("    BTFSC 0x{ta2:02X}, 7"));
        self.emit(format!("    BSF 0x{stick:02X}, 0"));
        self.emit(format!("    BTFSS 0x{ta2:02X}, 7"));
        self.emit(format!("    BCF 0x{stick:02X}, 0"));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_align_loop}"));
        self.emit(format!("{l_align_done}:"));
        // ---- signs equal? add : subtract ----
        self.emit(format!("    MOVF 0x{sa:02X}, W"));
        self.emit(format!("    XORWF 0x{sb:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sub}"));
        // add: ma += mb (3-byte carry chain); a carry renormalizes
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    ADDWF 0x{ma0:02X}, F"));
        self.emit(format!("    MOVF 0x{mb1:02X}, W"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{mb1:02X}, W"));
        self.emit(format!("    ADDWF 0x{ma1:02X}, F"));
        self.emit(format!("    MOVF 0x{mb2:02X}, W"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{mb2:02X}, W"));
        self.emit(format!("    ADDWF 0x{ma2:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_add_carry}"));
        self.emit(format!("    GOTO {l_round_step}"));
        self.emit(format!("{l_add_carry}:"));
        self.emit(format!("    BTFSC 0x{stick:02X}, 0"));
        self.emit(format!("    BSF 0x{stick:02X}, 1"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{ma2:02X}, F"));
        self.emit(format!("    RRF 0x{ma1:02X}, F"));
        self.emit(format!("    RRF 0x{ma0:02X}, F"));
        self.emit(format!("    BCF 0x{stick:02X}, 0"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{stick:02X}, 0"));
        self.emit(format!("    BSF 0x{ma2:02X}, 7"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{ea:02X}, F"));
        self.emit(format!("    GOTO {l_round_step}"));
        // subtract: compare ma vs mb (the sign follows the larger)
        self.emit(format!("{l_sub}:"));
        self.emit(format!("    MOVF 0x{mb2:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma2:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_cmp_b1}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b1}:"));
        self.emit(format!("    MOVF 0x{mb1:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma1:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_cmp_b0}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        self.emit(format!("{l_cmp_b0}:"));
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma0:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_sub_swap}"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_cmp_frac}"));
        self.emit(format!("    GOTO {l_sub_done}"));
        // ma == mb: |a| == |b| iff the fraction is 0, else a is larger by
        // exactly the fraction (the value is frac, sign = sa).
        self.emit(format!("{l_cmp_frac}:"));
        self.emit(format!("    MOVF 0x{ta0:02X}, W"));
        self.emit(format!("    IORWF 0x{ta1:02X}, W"));
        self.emit(format!("    IORWF 0x{ta2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sub_equal_frac}"));
        self.emit(format!("    BTFSS 0x{stick:02X}, 1"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_sub_equal_frac}:"));
        self.emit(format!("    BTFSC 0x{stick:02X}, 1"));
        self.emit(format!("    BSF 0x{ta0:02X}, 0"));
        self.emit(format!("    CLRF 0x{ma0:02X}"));
        self.emit(format!("    CLRF 0x{ma1:02X}"));
        self.emit(format!("    CLRF 0x{ma2:02X}"));
        self.emit(format!("    GOTO {l_normalize}"));
        self.emit(format!("{l_sub_swap}:"));
        for (x, y) in [(ma0, mb0), (ma1, mb1), (ma2, mb2)] {
            self.emit_xor_swap(x, y);
        }
        self.emit(format!("    MOVF 0x{sb:02X}, W"));
        self.emit(format!("    MOVWF 0x{sa:02X}"));
        self.emit(format!("{l_sub_done}:"));
        // ---- fractional borrow: the exact result is (ma - mb) - frac, so
        //      for frac != 0 the integer part borrows (ma -= 1) and the
        //      fraction becomes 2^24 - ta (the deep OR folded into ta's
        //      LSB first, it is below the 24-bit window, sticky-typed).
        //      frac == 0 skips straight to the plain 3-byte subtract. ----
        self.emit(format!("    BTFSC 0x{stick:02X}, 1"));
        self.emit(format!("    BSF 0x{ta0:02X}, 0"));
        self.emit(format!("    MOVF 0x{ta0:02X}, W"));
        self.emit(format!("    IORWF 0x{ta1:02X}, W"));
        self.emit(format!("    IORWF 0x{ta2:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sub_no_frac}"));
        self.emit(format!("    COMF 0x{ta0:02X}, F"));
        self.emit(format!("    COMF 0x{ta1:02X}, F"));
        self.emit(format!("    COMF 0x{ta2:02X}, F"));
        self.emit(format!("    INCF 0x{ta0:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{ta1:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{ta2:02X}, F"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma0:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma1:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_sub_borrow_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ma2:02X}, F"));
        self.emit(format!("{l_sub_borrow_done}:"));
        self.emit(format!("{l_sub_no_frac}:"));
        // ma -= mb (3-byte borrow chain)
        self.emit(format!("    MOVF 0x{mb0:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma0:02X}, F"));
        self.emit(format!("    MOVF 0x{mb1:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{mb1:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma1:02X}, F"));
        self.emit(format!("    MOVF 0x{mb2:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{mb2:02X}, W"));
        self.emit(format!("    SUBWF 0x{ma2:02X}, F"));
        self.emit(format!("    GOTO {l_normalize}"));
        // ---- normalize the 6-byte value: while !(ma2 bit 7) && ea > 1:
        //      (ta:ma) <<= 1 (the fraction's bits move into the mantissa),
        //      ea--. Only the subtract path reaches this (a sum never
        //      normalizes). The loop stops at ea == 1, NOT 0: a denormal
        //      result is the raw fraction at the exp-1 scale (value =
        //      frac x 2^-149), and the denormal conversion below drops it
        //      to exp 0. Stopping at 0 would leave ma = 2 x frac, doubling
        //      the stored value. ----
        self.emit(format!("{l_normalize}:"));
        self.emit(format!("    MOVF 0x{ma2:02X}, W"));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit("    SUBLW 0x01".to_string()); // ea == 1 -> stop
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sub_guard}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{ta0:02X}, F"));
        self.emit(format!("    RLF 0x{ta1:02X}, F"));
        self.emit(format!("    RLF 0x{ta2:02X}, F"));
        self.emit(format!("    RLF 0x{ma0:02X}, F"));
        self.emit(format!("    RLF 0x{ma1:02X}, F"));
        self.emit(format!("    RLF 0x{ma2:02X}, F"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{ea:02X}, F"));
        self.emit(format!("    GOTO {l_normalize}"));
        // ---- subtract-path guard: the top fraction bit (ta2 bit 7) ----
        self.emit(format!("{l_sub_guard}:"));
        self.emit(format!("    BTFSC 0x{ta2:02X}, 7"));
        self.emit(format!("    BSF 0x{stick:02X}, 0"));
        self.emit(format!("    BTFSS 0x{ta2:02X}, 7"));
        self.emit(format!("    BCF 0x{stick:02X}, 0"));
        // ---- RNE: round up iff round && (sticky || mantissa LSB), with
        //      sticky = OR(ta below the top bit) | stick bit 1 (the deep
        //      OR): the add path's round/sticky are equivalent (round =
        //      ta2 bit 7 = the last shifted-out bit; a sum carry promotes
        //      the old round into stick bit 1). ----
        // ---- denormal result: exp 1 with the top mantissa bit clear (the
        //      sum/difference of denormals, or a normal that underflowed)
        //      converts to exp 0: the mantissa is already the raw
        //      fraction (no implicit bit), and the assemble emits it as-is
        //      for e == 0. A rounded-up 0x800000 (the smallest normal)
        //      keeps exp 1. Both paths reach here: the add path directly
        //      (a denormal sum never normalizes), the sub path after its
        //      normalize loop stops at ea == 1. ----
        let l_den_conv = self.fresh_label();
        let l_den_done = self.fresh_label();
        self.emit(format!("{l_round_step}:"));
        self.emit(format!("    BTFSS 0x{stick:02X}, 0"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("    MOVF 0x{ta0:02X}, W"));
        self.emit(format!("    IORWF 0x{ta1:02X}, W"));
        self.emit(format!("    IORWF 0x{ta2:02X}, W"));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{stick:02X}, 1"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{ma0:02X}, 0"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(ma0, ma1, ma2, ea);
        self.emit(format!("{l_den_conv}:"));
        self.emit(format!("    MOVF 0x{ma2:02X}, W"));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_den_done}"));
        self.emit(format!("    CLRF 0x{ea:02X}"));
        self.emit(format!("{l_den_done}:"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sa, ea, ma0, ma1, ma2);
        // ---- zero result: sign = sa & sb, exp 0, mantissa 0 ----
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    BTFSS 0x{sa:02X}, 7"));
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit(format!("    BTFSS 0x{sb:02X}, 7"));
        self.emit(format!("    GOTO {l_zs_clear}"));
        self.emit(format!("    GOTO {l_zs_done}"));
        self.emit(format!("{l_zs_clear}:"));
        self.emit(format!("    BCF 0x{sa:02X}, 7"));
        self.emit(format!("{l_zs_done}:"));
        self.emit(format!("    CLRF 0x{ea:02X}"));
        self.emit(format!("    CLRF 0x{ma0:02X}"));
        self.emit(format!("    CLRF 0x{ma1:02X}"));
        self.emit(format!("    CLRF 0x{ma2:02X}"));
        self.emit(format!("    GOTO {l_assemble}"));
    }

    /// The __mul_f32 body (scratch: sign@0, e@1-2 = the 16-bit result exp,
    /// bk@3-5 = the multiplier backup, cnt@6 = 24, m@7-10 = the product's
    /// top 25 bits (P >> 23), low@11-13 = the exact mod-2^23 product low
    /// bits; the a/b param slots hold the shifted multiplicand addend and
    /// the low-part register). The 24x24 shift-add runs 24 iterations (the
    /// AN526 pattern): per set multiplier bit, low += (ma << i) mod 2^23
    /// (exact, its carry feeds m) and m += (ma >> (23-i)); then normalize
    /// (bit 47 of the product -> exp+1), round RNE (guard = P bit 22 /
    /// shifted-out bit, sticky = the low bits), assemble.
    fn emit_f32_mul_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (sign, e) = (scr, scr + 1);
        let (bk0, bk1, bk2) = (scr + 3, scr + 4, scr + 5);
        let cnt = scr + 6;
        let (m0, m1, m2, m3) = (scr + 7, scr + 8, scr + 9, scr + 10);
        let (low0, low1, low2) = (scr + 11, scr + 12, scr + 13);
        let l_loop = self.fresh_label();
        let l_skip = self.fresh_label();
        let l_renorm = self.fresh_label();
        let l_norm_check = self.fresh_label();
        let l_norm_left = self.fresh_label();
        let l_norm_left_shift = self.fresh_label();
        let l_norm_right = self.fresh_label();
        let l_extract = self.fresh_label();
        let l_den_conv = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_carry_in = self.fresh_label();
        let l_no_carry = self.fresh_label();
        let l_ehi_c1clear = self.fresh_label();
        let l_ehi_done = self.fresh_label();
        let l_a_exp_done = self.fresh_label();
        let l_b_exp_done = self.fresh_label();
        let l_a_nz = self.fresh_label();
        let l_b_nz = self.fresh_label();
        let l_a_not_ff = self.fresh_label();
        let l_a_inf = self.fresh_label();
        let l_b_not_ff = self.fresh_label();
        let l_b_inf = self.fresh_label();
        let l_a_inf_b_finite = self.fresh_label();
        let l_b_inf_a_finite = self.fresh_label();
        let l_a_mant_implicit = self.fresh_label();
        let l_a_mant_done = self.fresh_label();
        let l_b_mant_implicit = self.fresh_label();
        let l_b_mant_done = self.fresh_label();
        let l_zero = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_inf = self.fresh_label();
        // sign = (a3 ^ b3) & 0x80
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit(format!("    XORWF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{sign:02X}"));
        // e = ea + eb - 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8 + eb8 (9 bits: S_lo +
        // C0); e_lo = S_lo + 0x81 (C1); e_hi = C0 - borrow (borrow = !C1).
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{low0:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{low0:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    BSF 0x{low0:02X}, 0")); // ea8
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{low1:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{low1:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    BSF 0x{low1:02X}, 0")); // eb8
                                                       // A nonzero exp-zero operand aligns at exp 1 with its raw fraction.
        self.emit(format!("    MOVF 0x{low0:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 1));
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{low0:02X}"));
        self.emit(format!("{l_a_exp_done}:"));
        self.emit(format!("    MOVF 0x{low1:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 1));
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_exp_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{low1:02X}"));
        self.emit(format!("{l_b_exp_done}:"));
        self.emit(format!("    MOVF 0x{low1:02X}, W"));
        self.emit(format!("    ADDWF 0x{low0:02X}, W"));
        self.emit(format!("    MOVWF 0x{low0:02X}"));
        self.emit(format!("    CLRF 0x{m3:02X}"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{m3:02X}, 0")); // m3 bit 0 = C0
        self.emit("    MOVLW 0x81".to_string());
        self.emit(format!("    ADDWF 0x{low0:02X}, W"));
        self.emit(format!("    MOVWF 0x{e:02X}"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_ehi_c1clear}"));
        self.emit(format!("    BTFSC 0x{m3:02X}, 0"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_c1clear}:"));
        self.emit(format!("    BTFSC 0x{m3:02X}, 0"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit(format!("    MOVWF 0x{:02X}", e + 1));
        // NaN and infinity classification uses the raw exponent/fraction.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_b_not_ff}:"));
        // Finite zero operands produce signed zero; check the complete raw
        // fraction so denormals are not mistaken for zero.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{low2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{low2:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sign);
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sign);
        self.emit(format!("{l_b_nz}:"));
        // Normal operands receive the implicit bit; denormals retain raw
        // fractions (their exponents were bumped to one above).
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_a_mant_implicit}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", pa + 2));
        self.emit(format!("    GOTO {l_a_mant_done}"));
        self.emit(format!("{l_a_mant_implicit}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", pa + 2));
        self.emit(format!("{l_a_mant_done}:"));
        // bk = mb copy (the multiplier, shifted to test bits)
        // bk = mb copy (the multiplier, shifted to test bits)
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    MOVWF 0x{bk0:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{bk1:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{bk2:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_b_mant_implicit}"));
        self.emit(format!("    GOTO {l_b_mant_done}"));
        self.emit(format!("{l_b_mant_implicit}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{bk2:02X}"));
        self.emit(format!("{l_b_mant_done}:"));
        // la = the low-part addend, maintained as la_{i+1} = (la_i >> 1) |
        // (ma bit i << 22): the correct low contribution (ma mod 2^i) <<
        // (23-i) at iteration i (testing mb bit 23-i). Starts at 0 (i=0:
        // (ma mod 1) << 23 = 0). (The M15 float probe: an earlier attempt
        // copied ma into the slot: (ma mod 2^23) << i, which is a
        // different, wrong addend that broke every inexact product.)
        self.emit(format!("    CLRF 0x{:02X}", pb));
        self.emit(format!("    CLRF 0x{:02X}", pb + 1));
        self.emit(format!("    CLRF 0x{:02X}", pb + 2));
        for addr in [m0, m1, m2, m3, low0, low1, low2] {
            self.emit(format!("    CLRF 0x{addr:02X}"));
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_loop}:"));
        // test the multiplier bit (bk <<= 1, C = the bit)
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{bk0:02X}, F"));
        self.emit(format!("    RLF 0x{bk1:02X}, F"));
        self.emit(format!("    RLF 0x{bk2:02X}, F"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_skip}"));
        // low += la (3-byte): the FIRST byte adds WITHOUT a carry-in: the
        // C at this point is the tested multiplier bit (set by the RLF bk
        // chain), not a carry, so the BTFSC/INCFSZ carry-in would add a
        // spurious +1 per set-bit iteration (the M15 float probe found the
        // low sum came out one per set bit too high). Bytes 1-2 take the
        // carry from the previous byte's add.
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    ADDWF 0x{low0:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 1));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", pb + 1));
        self.emit(format!("    ADDWF 0x{low1:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", pb + 2));
        self.emit(format!("    ADDWF 0x{low2:02X}, F"));
        // m += addend (4-byte) + the low's carry-out: the carry into m is
        // BIT 23 of the 24-bit low sum (the top byte's bit 7), NOT the
        // byte carry-out (bit 24): the M15 float probe found the original
        // tested STATUS C, so a sum with bit 23 set but no byte overflow
        // (e.g. 0x700003 + 0x160000 = 0x860003) lost its carry into m and
        // every inexact product came out one 2^23 short. The carry path
        // also masks bit 23 out of low (low is mod 2^23).
        self.emit(format!("    BTFSC 0x{low2:02X}, 7"));
        self.emit(format!("    GOTO {l_carry_in}"));
        self.emit(format!("    GOTO {l_no_carry}"));
        self.emit(format!("{l_carry_in}:"));
        self.emit(format!("    BCF 0x{low2:02X}, 7"));
        self.emit(format!("    INCF 0x{m0:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{m1:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{m2:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{m3:02X}, F"));
        self.emit(format!("{l_no_carry}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    ADDWF 0x{m0:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 1));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", pa + 1));
        self.emit(format!("    ADDWF 0x{m1:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", pa + 2));
        self.emit(format!("    ADDWF 0x{m2:02X}, F"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{m3:02X}, F"));
        self.emit(format!("{l_skip}:"));
        // la = (la >> 1) | (ma bit i << 22): pa bit 0 is ma bit i (pa has
        // been shifted right i times), so the new bit enters at la bit 22.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pb + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pb + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pb));
        self.emit(format!("    BTFSC 0x{:02X}, 0", pa));
        self.emit(format!("    BSF 0x{:02X}, 6", pb + 2));
        // addend >>= 1 (pa = ma >> (i+1))
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        // Convert the product into a unified 47-bit register. A renormalized
        // product already has the correct scale after m >>= 1; otherwise the
        // leading zero m3 is dropped by shifting P left once.
        self.emit(format!("    BTFSC 0x{m3:02X}, 0"));
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit("    BCF STATUS, 0".to_string());
        // The m bytes already occupy P bits 46..23; shift only the low
        // 23-bit portion so P bit 22 becomes the unified guard bit.
        self.emit(format!("    RLF 0x{low0:02X}, F"));
        self.emit(format!("    RLF 0x{low1:02X}, F"));
        self.emit(format!("    RLF 0x{low2:02X}, F"));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_renorm}:"));
        // m >>= 1; the old m bit 0 is the unified register's guard bit.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    BTFSC 0x{m3:02X}, 0"));
        self.emit("    BSF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{m2:02X}, F"));
        self.emit(format!("    RRF 0x{m1:02X}, F"));
        self.emit(format!("    RRF 0x{m0:02X}, F"));
        self.emit(format!("    BCF 0x{low2:02X}, 7"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{low2:02X}, 7"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{e:02X}, F"));
        self.emit(format!("{l_norm_check}:"));
        // First handle e < 1 (including the negative 16-bit exponents of
        // tiny products), then left-normalize while e > 1.
        self.emit(format!("    BTFSC 0x{:02X}, 7", e + 1));
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit(format!("    MOVF 0x{:02X}, W", e + 1));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_norm_right}"));
        self.emit(format!("    GOTO {l_norm_left}"));
        self.emit(format!("{l_norm_left}:"));
        self.emit(format!("    BTFSC 0x{m2:02X}, 7"));
        self.emit(format!("    GOTO {l_extract}"));
        self.emit(format!("    MOVF 0x{:02X}, W", e + 1));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_norm_left_shift}"));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_extract}"));
        self.emit(format!("{l_norm_left_shift}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{low0:02X}, F"));
        self.emit(format!("    RLF 0x{low1:02X}, F"));
        self.emit(format!("    RLF 0x{low2:02X}, F"));
        self.emit(format!("    RLF 0x{m0:02X}, F"));
        self.emit(format!("    RLF 0x{m1:02X}, F"));
        self.emit(format!("    RLF 0x{m2:02X}, F"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:02X}, F", e + 1));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_norm_right}:"));
        self.emit(format!("    BTFSC 0x{low0:02X}, 0"));
        self.emit(format!("    BSF 0x{m3:02X}, 1"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{m2:02X}, F"));
        self.emit(format!("    RRF 0x{m1:02X}, F"));
        self.emit(format!("    RRF 0x{m0:02X}, F"));
        self.emit(format!("    RRF 0x{low2:02X}, F"));
        self.emit(format!("    RRF 0x{low1:02X}, F"));
        self.emit(format!("    RRF 0x{low0:02X}, F"));
        self.emit(format!("    INCF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", e + 1));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_extract}:"));
        // guard = unified bit 23; sticky = unified bits 0..22 plus any bits
        // shifted out while producing a denormal.
        self.emit(format!("    BTFSS 0x{low2:02X}, 7"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit("    MOVLW 0x7F".to_string());
        self.emit(format!("    ANDWF 0x{low2:02X}, W"));
        self.emit(format!("    IORWF 0x{low1:02X}, W"));
        self.emit(format!("    IORWF 0x{low0:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{m3:02X}, 1"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{m0:02X}, 0"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(m0, m1, m2, e);
        self.emit(format!("{l_den_conv}:"));
        // exp 1 with a clear mantissa top is the denormal encoding.
        self.emit(format!("    BTFSC 0x{m2:02X}, 7"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{:02X}, W", e + 1));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    CLRF 0x{e:02X}"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, m0, m1, m2);
        // the +/-0 result (zero operand): sign | 0
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    CLRF 0x{:02X}", self.retval_lo));
        self.emit(format!("    CLRF 0x{:02X}", self.retval_lo + 1));
        self.emit(format!("    CLRF 0x{:02X}", self.retval_lo + 2));
        self.emit(format!("    CLRF 0x{:02X}", self.retval_lo + 3));
        self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
        self.emit(format!("    BSF 0x{:02X}, 7", self.retval_lo + 3));
        self.emit("    RETURN".to_string());
    }

    /// One restoring-division compare/subtract/restore step: rem (4 bytes)
    /// -= den (3 bytes, the top byte is implicitly 0) with the borrow
    /// folds; on underflow (rem < den) add den back. The final C is the
    /// quotient bit: the caller's branch lands at `l_restore` when clear and
    /// sets the bit at `qbit` bit 0 otherwise; `l_next` resumes after.
    fn emit_f32_div_step(&mut self, rem: u16, den: u16, qbit: u16, l_restore: &str, l_next: &str) {
        self.emit(format!("    MOVF 0x{den:02X}, W"));
        self.emit(format!("    SUBWF 0x{rem:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1));
        self.emit(format!("    SUBWF 0x{:02X}, F", rem + 1));
        self.emit(format!("    MOVF 0x{:02X}, W", den + 2));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", den + 2));
        self.emit(format!("    SUBWF 0x{:02X}, F", rem + 2));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:02X}, F", rem + 3));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_restore}"));
        self.emit(format!("    BSF 0x{qbit:02X}, 0"));
        self.emit(format!("    GOTO {l_next}"));
        self.emit(format!("{l_restore}:"));
        self.emit(format!("    MOVF 0x{den:02X}, W"));
        self.emit(format!("    ADDWF 0x{rem:02X}, F"));
        self.emit(format!("    MOVF 0x{:02X}, W", den + 1));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", den + 1));
        self.emit(format!("    ADDWF 0x{:02X}, F", rem + 1));
        self.emit(format!("    MOVF 0x{:02X}, W", den + 2));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCFSZ 0x{:02X}, W", den + 2));
        self.emit(format!("    ADDWF 0x{:02X}, F", rem + 2));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit("    ADDLW 0x01".to_string());
        self.emit(format!("    ADDWF 0x{:02X}, F", rem + 3));
        self.emit(format!("{l_next}:"));
    }

    /// The __div_f32 body (scratch: sign@0, e@1-2 = the 16-bit result exp,
    /// rem@3-6 = the partial remainder, den@7-9 = the denominator copy, cnt@10,
    /// spare@11). The numerator lives in the a param slot. 24 restoring
    /// iterations give floor(ma/mb) (0/1, ma >= mb bumps e) with rem = ma
    /// mod mb; 25 more iterations extend the quotient to the mantissa +
    /// guard (the remainder at the end is the sticky). Div-by-zero (mb == 0)
    /// -> +/-infinity (0x7F800000, deterministic, documented); ma == 0 ->
    /// +/-0. The e is clamped by the byte arithmetic for the out-of-range
    /// cases (deterministic; the acceptance stays in the normal range).
    fn emit_f32_div_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (sign, e) = (scr, scr + 1);
        let (rem0, rem1, rem2, rem3) = (scr + 3, scr + 4, scr + 5, scr + 6);
        let (den0, den1, den2) = (scr + 7, scr + 8, scr + 9);
        let cnt = scr + 10;
        let spare = scr + 11;
        let r = self.retval_lo;
        let l_zero = self.fresh_label();
        let l_inf = self.fresh_label();
        let l_nan = self.fresh_label();
        let l_loop = self.fresh_label();
        let l_restore = self.fresh_label();
        let l_next = self.fresh_label();
        let l_ge1 = self.fresh_label();
        let l_qsave = self.fresh_label();
        let l_floop = self.fresh_label();
        let l_frestore = self.fresh_label();
        let l_fnext = self.fresh_label();
        let l_round = self.fresh_label();
        let l_round_test = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        let l_den_conv = self.fresh_label();
        let l_den_shift = self.fresh_label();
        let l_norm_a = self.fresh_label();
        let l_norm_a_done = self.fresh_label();
        let l_norm_b = self.fresh_label();
        let l_norm_b_done = self.fresh_label();
        let l_ehi_b = self.fresh_label();
        let l_ehi_done = self.fresh_label();
        let l_a_not_ff = self.fresh_label();
        let l_a_inf = self.fresh_label();
        let l_b_not_ff = self.fresh_label();
        let l_b_inf = self.fresh_label();
        let l_a_inf_b_finite = self.fresh_label();
        let l_b_inf_a_finite = self.fresh_label();
        let l_a_nz = self.fresh_label();
        let l_a_imp = self.fresh_label();
        let l_a_ready = self.fresh_label();
        let l_b_imp = self.fresh_label();
        let l_b_ready = self.fresh_label();
        let l_b_nz = self.fresh_label();
        let l_exp_a_done = self.fresh_label();
        let l_exp_b_done = self.fresh_label();
        let l_e_sub_done = self.fresh_label();
        // sign = (a3 ^ b3) & 0x80
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit(format!("    XORWF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{sign:02X}"));
        // e = ea - eb + 127 (16-bit) with the FULL 8-bit biased exponents
        // ((b3 & 0x7F) << 1 | (b2 >> 7)). S = ea8 - eb8 (S_lo + borrow B);
        // e_lo = S_lo + 0x7F (C1); e_hi = C1 - B.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{spare:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    BSF 0x{spare:02X}, 0")); // ea8
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{e:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    BSF 0x{e:02X}, 0")); // eb8
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit(format!("    SUBWF 0x{spare:02X}, W"));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    CLRF 0x{rem3:02X}"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{rem3:02X}, 0")); // rem3 bit 0 = borrow B
        self.emit("    ADDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:02X}"));
        self.emit("    MOVLW 0x00".to_string());
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_ehi_b}"));
        self.emit(format!("    BTFSC 0x{rem3:02X}, 0"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit(format!("{l_ehi_b}:"));
        self.emit(format!("    BTFSS 0x{rem3:02X}, 0"));
        self.emit(format!("    GOTO {l_ehi_done}"));
        self.emit("    MOVLW 0xFF".to_string());
        self.emit(format!("{l_ehi_done}:"));
        self.emit(format!("    MOVWF 0x{:02X}", e + 1));
        // IEEE class dispatch, using the raw exponent and complete fraction.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_a_not_ff}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_not_ff}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_b_not_ff}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_inf}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_inf}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    XORLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_a_inf_b_finite}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_inf_b_finite}:"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_b_inf}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_b_inf_a_finite}"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_inf_a_finite}:"));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("{l_b_not_ff}:"));
        // finite zero checks include the complete fraction
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_nz}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_zero}"));
        self.emit(format!("    GOTO {l_nan}"));
        self.emit(format!("{l_a_nz}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{spare:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_nz}"));
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_nan}:"));
        self.emit_f32_nan(sign);
        self.emit(format!("{l_inf}:"));
        self.emit_f32_inf(sign);
        self.emit(format!("{l_zero}:"));
        self.emit(format!("    CLRF 0x{r:02X}"));
        self.emit(format!("    CLRF 0x{:02X}", r + 1));
        self.emit(format!("    CLRF 0x{:02X}", r + 2));
        self.emit(format!("    CLRF 0x{:02X}", r + 3));
        self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
        self.emit(format!("    BSF 0x{:02X}, 7", r + 3));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_b_nz}:"));
        // Denormals begin at the exp-1 alignment scale (effective e8 = 1).
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_exp_a_done}"));
        self.emit(format!("    INCF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", e + 1));
        self.emit(format!("{l_exp_a_done}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_exp_b_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:02X}, F", e + 1));
        self.emit(format!("{l_exp_b_done}:"));
        // Build raw/implicit mantissas, then normalize denormals to bit 23.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", pa + 2));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_a_imp}"));
        self.emit(format!("    GOTO {l_a_ready}"));
        self.emit(format!("{l_a_imp}:"));
        self.emit(format!("    BSF 0x{:02X}, 7", pa + 2));
        self.emit(format!("{l_a_ready}:"));
        self.emit(format!("    CLRF 0x{rem3:02X}"));
        self.emit(format!("{l_norm_a}:"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_norm_a_done}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{:02X}, F", pa));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 2));
        self.emit(format!("    INCF 0x{rem3:02X}, F"));
        self.emit(format!("    GOTO {l_norm_a}"));
        self.emit(format!("{l_norm_a_done}:"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", pb + 2));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_b_imp}"));
        self.emit(format!("    GOTO {l_b_ready}"));
        self.emit(format!("{l_b_imp}:"));
        self.emit(format!("    BSF 0x{:02X}, 7", pb + 2));
        self.emit(format!("{l_b_ready}:"));
        self.emit(format!("    CLRF 0x{cnt:02X}"));
        self.emit(format!("{l_norm_b}:"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_norm_b_done}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{:02X}, F", pb));
        self.emit(format!("    RLF 0x{:02X}, F", pb + 1));
        self.emit(format!("    RLF 0x{:02X}, F", pb + 2));
        self.emit(format!("    INCF 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_norm_b}"));
        self.emit(format!("{l_norm_b_done}:"));
        self.emit(format!("    MOVF 0x{rem3:02X}, W"));
        self.emit(format!("    SUBWF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_e_sub_done}"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{:02X}, F", e + 1));
        self.emit(format!("{l_e_sub_done}:"));
        self.emit(format!("    MOVF 0x{cnt:02X}, W"));
        self.emit(format!("    ADDWF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", e + 1));
        // denominator copy, now normalized to [2^23, 2^24).
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    MOVWF 0x{den0:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 1));
        self.emit(format!("    MOVWF 0x{den1:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit(format!("    MOVWF 0x{den2:02X}"));
        // ---- 24 restoring iterations: num <<= 1; rem = rem << 1 | C;
        //      if rem >= den set the quotient bit else restore ----
        for addr in [rem0, rem1, rem2, rem3] {
            self.emit(format!("    CLRF 0x{addr:02X}"));
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_loop}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{:02X}, F", pa));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RLF 0x{rem0:02X}, F"));
        self.emit(format!("    RLF 0x{rem1:02X}, F"));
        self.emit(format!("    RLF 0x{rem2:02X}, F"));
        self.emit(format!("    RLF 0x{rem3:02X}, F"));
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_restore, &l_next);
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        // Save floor(ma/mb) (0/1, ma >= mb) before the mantissa
        // accumulator clears pa.
        self.emit(format!("    CLRF 0x{spare:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit("    ANDLW 0x01".to_string());
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_qsave}"));
        self.emit(format!("    BSF 0x{spare:02X}, 0"));
        self.emit(format!("{l_qsave}:"));
        // ---- 25 more iterations: the mantissa + guard, with the sticky in
        //      the remainder ----
        for addr in [pa, pa + 1, pa + 2, pa + 3] {
            self.emit(format!("    CLRF 0x{addr:02X}"));
        }
        self.emit("    MOVLW 0x19".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_floop}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{:02X}, F", pa));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RLF 0x{:02X}, F", pa + 3));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{rem0:02X}, F"));
        self.emit(format!("    RLF 0x{rem1:02X}, F"));
        self.emit(format!("    RLF 0x{rem2:02X}, F"));
        self.emit(format!("    RLF 0x{rem3:02X}, F"));
        self.emit_f32_div_step(scr + 3, scr + 7, pa, &l_frestore, &l_fnext);
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_floop}"));
        // The mantissa: q = ma/mb in [0.5, 2). The fraction loop's 25 bits
        // are q's bits 2^-1..2^-25 (pa: f1 at bit 24 .. f25 at bit 0), with
        // the remainder as the sticky. For q < 1 (floor(q) == 0) the
        // mantissa = f1..f24 (f1 = 1 at bit 23) with exp-1; for q >= 1 the
        // mantissa = 1.f1..f23 = 0x800000 | (pa >> 2) with the guard f24
        // (pa bit 1) and the sticky f25 (pa bit 0) | rem. e = ea - eb + 127.
        self.emit(format!("    BTFSC 0x{spare:02X}, 0"));
        self.emit(format!("    GOTO {l_ge1}"));
        // q < 1: mantissa = pa >> 1; guard = old pa bit 0; sticky = rem;
        // exp -= 1.
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    SUBWF 0x{e:02X}, F"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    BTFSC 0x{:02X}, 0", pa + 3));
        self.emit("    BSF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit(format!("    CLRF 0x{spare:02X}"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{spare:02X}, 0")); // guard
        self.emit(format!("    MOVF 0x{rem0:02X}, W"));
        self.emit(format!("    IORWF 0x{rem1:02X}, W"));
        self.emit(format!("    IORWF 0x{rem2:02X}, W"));
        self.emit(format!("    IORWF 0x{rem3:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("    BSF 0x{spare:02X}, 1")); // sticky
        self.emit(format!("    GOTO {l_round}"));
        // q >= 1: mantissa = 0x800000 | (pa >> 2); guard = old pa bit 1;
        // sticky = old pa bit 0 | rem.
        self.emit(format!("{l_ge1}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    BTFSC 0x{:02X}, 0", pa + 3));
        self.emit("    BSF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit(format!("    CLRF 0x{spare:02X}"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{spare:02X}, 1")); // old bit 0 -> sticky
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{spare:02X}, 0")); // guard = old bit 1
        self.emit(format!("    BSF 0x{:02X}, 7", pa + 2)); // the leading 1
        self.emit(format!("    MOVF 0x{rem0:02X}, W"));
        self.emit(format!("    IORWF 0x{rem1:02X}, W"));
        self.emit(format!("    IORWF 0x{rem2:02X}, W"));
        self.emit(format!("    IORWF 0x{rem3:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("    BSF 0x{spare:02X}, 1")); // sticky |= rem
                                                        // RNE: guard (spare bit 0) && (sticky (spare bit 1) || mantissa LSB)
        self.emit(format!("{l_round}:"));
        // Shift a subnormal result right while e < 1, preserving guard and
        // sticky for the final round-to-nearest-even decision.
        self.emit(format!("    BTFSC 0x{:02X}, 7", e + 1));
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round_test}"));
        self.emit(format!("    GOTO {l_den_shift}"));
        self.emit(format!("{l_den_shift}:"));
        self.emit(format!("    BTFSC 0x{spare:02X}, 0"));
        self.emit(format!("    BSF 0x{spare:02X}, 1"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit(format!("    BCF 0x{spare:02X}, 0"));
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit(format!("    BSF 0x{spare:02X}, 0"));
        self.emit(format!("    INCF 0x{e:02X}, F"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    INCF 0x{:02X}, F", e + 1));
        self.emit(format!("    GOTO {l_round}"));
        self.emit(format!("{l_round_test}:"));
        self.emit(format!("    BTFSS 0x{spare:02X}, 0"));
        self.emit(format!("    GOTO {l_den_conv}"));
        self.emit(format!("    BTFSC 0x{spare:02X}, 1"));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{:02X}, 0", pa));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(pa, pa + 1, pa + 2, e);
        self.emit(format!("{l_den_conv}:"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{:02X}, W", e + 1));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    SUBLW 0x01".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    CLRF 0x{e:02X}"));
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, pa, pa + 1, pa + 2);
    }

    /// The __uitofp_f32 body (scratch: cnt@0, e@1-2, guard@3, stick@4,
    /// spare@5-7; `sign_src` = the byte holding the sign for __sitofp_f32,
    /// or None for the unsigned +0). Leading-1 search: shift the value left
    /// until bit 31 set, counting; e = 127 + 31 - shifts; the mantissa is
    /// the shifted value's top 24 bits, guard = bit 7 of the low byte,
    /// sticky = its low 7 bits. Round RNE.
    fn emit_f32_uitofp_body(&mut self, val: u16, scr: u16, sign_src: Option<u16>) {
        let (cnt, e) = (scr, scr + 1);
        let (guard, stick) = (scr + 3, scr + 4);
        let sign = scr + 5;
        let r = self.retval_lo;
        let l_zero = self.fresh_label();
        let l_nz = self.fresh_label();
        let l_loop = self.fresh_label();
        let l_round_up = self.fresh_label();
        let l_assemble = self.fresh_label();
        // zero input -> +/-0 (the sign byte is 0 for uitofp)
        self.emit(format!("    MOVF 0x{:02X}, W", val));
        self.emit(format!("    IORWF 0x{:02X}, W", val + 1));
        self.emit(format!("    IORWF 0x{:02X}, W", val + 2));
        self.emit(format!("    IORWF 0x{:02X}, W", val + 3));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit(format!("    CLRF 0x{r:02X}"));
        self.emit(format!("    CLRF 0x{:02X}", r + 1));
        self.emit(format!("    CLRF 0x{:02X}", r + 2));
        self.emit(format!("    CLRF 0x{:02X}", r + 3));
        if sign_src.is_some() {
            self.emit(format!("    BTFSC 0x{sign:02X}, 7"));
            self.emit(format!("    BSF 0x{:02X}, 7", r + 3));
        }
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_nz}:"));
        if sign_src.is_none() {
            self.emit(format!("    CLRF 0x{sign:02X}"));
        }
        self.emit(format!("    CLRF 0x{cnt:02X}"));
        self.emit(format!("{l_loop}:"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", val + 3));
        self.emit(format!("    GOTO {l_zero}"));
        self.emit("    BCF STATUS, 0".to_string());
        for i in 0..4 {
            self.emit(format!("    RLF 0x{:02X}, F", val + i));
        }
        self.emit(format!("    INCF 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        self.emit(format!("{l_zero}:"));
        // e = 158 - cnt; the mantissa is val+1..val+3 (bit 23 = val3 bit 7)
        self.emit(format!("    MOVF 0x{cnt:02X}, W"));
        self.emit("    SUBLW 0x9E".to_string()); // 158 - cnt
        self.emit(format!("    MOVWF 0x{e:02X}"));
        self.emit(format!("    CLRF 0x{:02X}", e + 1));
        self.emit(format!("    MOVF 0x{:02X}, W", val));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{guard:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", val));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{stick:02X}"));
        // RNE: guard && (sticky || mantissa LSB)
        self.emit(format!("    BTFSS 0x{guard:02X}, 7"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("    MOVF 0x{stick:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    BTFSC 0x{:02X}, 0", val + 1));
        self.emit(format!("    GOTO {l_round_up}"));
        self.emit(format!("    GOTO {l_assemble}"));
        self.emit(format!("{l_round_up}:"));
        self.emit_f32_round_up(val + 1, val + 2, val + 3, e);
        self.emit(format!("{l_assemble}:"));
        self.emit_f32_assemble(sign, e, val + 1, val + 2, val + 3);
    }

    /// The __fptoui_f32 / __fptosi_f32 body (scratch: e@0, cnt@1, m@2-4,
    /// sign@5, spare@6 = the result's 4th byte). The biased exponent maps to
    /// a mantissa shift: right by 150 - e (truncating; e <= 150), left by
    /// e - 150 (e in 151..158, the u32 range), or a deterministic clamp
    /// (e >= 159 for fptoui -> 0xFFFFFFFF; e >= 158 for fptosi -> +/-0x7F800000
    /// style saturation). `signed` negates the result for a negative input
    /// (the sign is ignored for fptoui; truncation toward zero).
    fn emit_f32_fptoi_body(&mut self, val: u16, scr: u16, signed: bool) {
        let (e, cnt) = (scr, scr + 1);
        let (m0, m1, m2) = (scr + 2, scr + 3, scr + 4);
        let sign = scr + 5;
        let m3 = scr + 6;
        let r = self.retval_lo;
        let l_nz = self.fresh_label();
        let l_left = self.fresh_label();
        let l_rdone = self.fresh_label();
        let l_rloop = self.fresh_label();
        let l_lloop = self.fresh_label();
        let l_posclamp = self.fresh_label();
        let l_store2 = self.fresh_label();
        let l_store = self.fresh_label();
        // e = (b3 & 0x7F) << 1 | (b2 >> 7)
        self.emit(format!("    MOVF 0x{:02X}, W", val + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    MOVWF 0x{e:02X}"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{e:02X}, F"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", val + 2));
        self.emit(format!("    BSF 0x{e:02X}, 0"));
        if signed {
            self.emit(format!("    MOVF 0x{:02X}, W", val + 3));
            self.emit("    ANDLW 0x80".to_string());
            self.emit(format!("    MOVWF 0x{sign:02X}"));
        }
        // e == 0 -> result 0 (the sign is dropped for zero)
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nz}"));
        self.emit(format!("    CLRF 0x{m0:02X}"));
        self.emit(format!("    CLRF 0x{m1:02X}"));
        self.emit(format!("    CLRF 0x{m2:02X}"));
        self.emit(format!("    CLRF 0x{m3:02X}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_nz}:"));
        // m = the 24-bit mantissa with the implicit bit
        self.emit(format!("    MOVF 0x{:02X}, W", val));
        self.emit(format!("    MOVWF 0x{m0:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", val + 1));
        self.emit(format!("    MOVWF 0x{m1:02X}"));
        self.emit(format!("    MOVF 0x{:02X}, W", val + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    IORLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{m2:02X}"));
        self.emit(format!("    CLRF 0x{m3:02X}"));
        // cnt = 150 - e
        self.emit(format!("    MOVF 0x{e:02X}, W"));
        self.emit("    SUBLW 0x96".to_string()); // 150 - e
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_left}"));
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        // clamp the right count to 31 (the 24-bit mantissa is zero beyond)
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    SUBWF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_rdone}"));
        self.emit("    MOVLW 0x1F".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_rdone}:"));
        self.emit(format!("    MOVF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_rloop}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{m2:02X}, F"));
        self.emit(format!("    RRF 0x{m1:02X}, F"));
        self.emit(format!("    RRF 0x{m0:02X}, F"));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_rloop}"));
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_left}:"));
        // cnt = e - 150 (W = 150 - e, negate)
        self.emit("    SUBLW 0x00".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        // overflow clamp: fptoui cnt > 8 (e >= 159); fptosi cnt >= 8 (e >= 158)
        if signed {
            self.emit("    MOVLW 0x08".to_string());
        } else {
            self.emit("    MOVLW 0x09".to_string());
        }
        self.emit(format!("    SUBWF 0x{cnt:02X}, W"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_lloop}"));
        if signed {
            self.emit(format!("    BTFSS 0x{sign:02X}, 7"));
            self.emit(format!("    GOTO {l_posclamp}"));
            self.emit(format!("    CLRF 0x{m0:02X}"));
            self.emit(format!("    CLRF 0x{m1:02X}"));
            self.emit(format!("    CLRF 0x{m2:02X}"));
            self.emit("    MOVLW 0x80".to_string());
            self.emit(format!("    MOVWF 0x{m3:02X}"));
            self.emit(format!("    GOTO {l_store2}"));
            self.emit(format!("{l_posclamp}:"));
            self.emit("    MOVLW 0xFF".to_string());
            self.emit(format!("    MOVWF 0x{m0:02X}"));
            self.emit(format!("    MOVWF 0x{m1:02X}"));
            self.emit(format!("    MOVWF 0x{m2:02X}"));
            self.emit("    MOVLW 0x7F".to_string());
            self.emit(format!("    MOVWF 0x{m3:02X}"));
        } else {
            self.emit("    MOVLW 0xFF".to_string());
            self.emit(format!("    MOVWF 0x{m0:02X}"));
            self.emit(format!("    MOVWF 0x{m1:02X}"));
            self.emit(format!("    MOVWF 0x{m2:02X}"));
            self.emit(format!("    MOVWF 0x{m3:02X}"));
        }
        self.emit(format!("    GOTO {l_store2}"));
        self.emit(format!("{l_lloop}:"));
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{m0:02X}, F"));
        self.emit(format!("    RLF 0x{m1:02X}, F"));
        self.emit(format!("    RLF 0x{m2:02X}, F"));
        self.emit(format!("    RLF 0x{m3:02X}, F"));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_lloop}"));
        self.emit(format!("{l_store2}:"));
        if signed {
            // negate the 4-byte result for a negative input (truncation is
            // toward zero, the negate of 0 is 0)
            self.emit(format!("    BTFSS 0x{sign:02X}, 7"));
            self.emit(format!("    GOTO {l_store}"));
            for addr in [m0, m1, m2, m3] {
                self.emit(format!("    COMF 0x{addr:02X}, F"));
            }
            self.emit(format!("    INCF 0x{m0:02X}, F"));
            self.emit("    BTFSC STATUS, 2".to_string());
            self.emit(format!("    INCF 0x{m1:02X}, F"));
            self.emit("    BTFSC STATUS, 2".to_string());
            self.emit(format!("    INCF 0x{m2:02X}, F"));
            self.emit("    BTFSC STATUS, 2".to_string());
            self.emit(format!("    INCF 0x{m3:02X}, F"));
            self.emit(format!("{l_store}:"));
        }
        for (i, addr) in [m0, m1, m2, m3].iter().enumerate() {
            self.emit(format!("    MOVF 0x{addr:02X}, W"));
            self.emit(format!("    MOVWF 0x{:02X}", r + i as u16));
        }
        self.emit("    RETURN".to_string());
    }

    /// The __cmp_f32 body (scratch: tmp@0-1; the params are compared in
    /// place with the sign bits cleared for the magnitude compare). NaN
    /// (exp 0xFF, mantissa nonzero) -> 3; both zero (full 8-bit exp == 0,
    /// any signs) -> 0; the sign-magnitude ordering with the sign of the
    /// larger deciding lt/gt (negative values reverse the magnitude order).
    fn emit_f32_cmp_body(&mut self, pa: u16, pb: u16, scr: u16) {
        let (tmp0, tmp1) = (scr, scr + 1);
        let r = self.retval_lo;
        let l_nan_a_done = self.fresh_label();
        let l_nan_b_done = self.fresh_label();
        let l_ret3 = self.fresh_label();
        let l_az_done = self.fresh_label();
        let l_ret0 = self.fresh_label();
        let l_sign_diff = self.fresh_label();
        let l_ret1 = self.fresh_label();
        let l_ret2 = self.fresh_label();
        let l_mag_lt = self.fresh_label();
        // NaN a: (b3 & 0x7F) == 0x7F && b2 bit 7 && (b2&0x7F | b1 | b0) != 0
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_nan_a_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{:02X}, W", pa + 1));
        self.emit(format!("    IORWF 0x{:02X}, W", pa));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_a_done}:"));
        // NaN b
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    SUBLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_nan_b_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 2));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit(format!("    IORWF 0x{:02X}, W", pb + 1));
        self.emit(format!("    IORWF 0x{:02X}, W", pb));
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_ret3}"));
        self.emit(format!("{l_nan_b_done}:"));
        // both zero (full 8-bit exp == 0, any signs) -> equal. The exponent's
        // LSB lives in b2 bit 7, so the (b3 & 0x7F) test alone swallows the
        // smallest NORMALs (8-bit exp 1: 0x00800000..0x00FFFFFF): skip the
        // zero path when b2 bit 7 is set, mirroring the mul/div zero checks.
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pa + 2));
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    MOVF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", pb + 2));
        self.emit(format!("    GOTO {l_az_done}"));
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit(format!("{l_az_done}:"));
        // signs differ? a negative, b positive -> a < b (1); else a > b (2).
        // Mask the XOR to bit 7 (the exponent bits must not pollute it).
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit(format!("    XORWF 0x{:02X}, W", pb + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_sign_diff}"));
        // same sign: save a's sign, clear the sign bits, compare magnitudes
        self.emit(format!("    MOVF 0x{:02X}, W", pa + 3));
        self.emit("    ANDLW 0x80".to_string());
        self.emit(format!("    MOVWF 0x{tmp1:02X}"));
        self.emit(format!("    BCF 0x{:02X}, 7", pa + 3));
        self.emit(format!("    BCF 0x{:02X}, 7", pb + 3));
        // equality: OR-accumulate the byte XORs into tmp0
        self.emit(format!("    MOVF 0x{:02X}, W", pa));
        self.emit(format!("    XORWF 0x{:02X}, W", pb));
        self.emit(format!("    MOVWF 0x{tmp0:02X}"));
        for i in 1..4 {
            self.emit(format!("    MOVF 0x{:02X}, W", pa + i));
            self.emit(format!("    XORWF 0x{:02X}, W", pb + i));
            self.emit(format!("    IORWF 0x{tmp0:02X}, W"));
            self.emit(format!("    MOVWF 0x{tmp0:02X}"));
        }
        // the 4-byte unsigned compare chain: C = (pa >= pb)
        self.emit(format!("    MOVF 0x{:02X}, W", pb));
        self.emit(format!("    SUBWF 0x{:02X}, W", pa));
        for i in 1..4 {
            self.emit(format!("    MOVF 0x{:02X}, W", pb + i));
            self.emit("    BTFSS STATUS, 0".to_string());
            self.emit(format!("    INCFSZ 0x{:02X}, W", pb + i));
            self.emit(format!("    SUBWF 0x{:02X}, W", pa + i));
        }
        // equal -> 0; pa < pb -> (negative ? 2 : 1); pa > pb -> (negative ? 1 : 2)
        self.emit(format!("    MOVF 0x{tmp0:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_ret0}"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_mag_lt}"));
        self.emit(format!("    BTFSS 0x{tmp1:02X}, 7"));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_mag_lt}:"));
        self.emit(format!("    BTFSS 0x{tmp1:02X}, 7"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("{l_sign_diff}:"));
        self.emit(format!("    BTFSS 0x{:02X}, 7", pa + 3));
        self.emit(format!("    GOTO {l_ret2}"));
        self.emit(format!("    GOTO {l_ret1}"));
        self.emit(format!("{l_ret0}:"));
        self.emit(format!("    CLRF 0x{r:02X}"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret1}:"));
        self.emit("    MOVLW 0x01".to_string());
        self.emit(format!("    MOVWF 0x{r:02X}"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret2}:"));
        self.emit("    MOVLW 0x02".to_string());
        self.emit(format!("    MOVWF 0x{r:02X}"));
        self.emit("    RETURN".to_string());
        self.emit(format!("{l_ret3}:"));
        self.emit("    MOVLW 0x03".to_string());
        self.emit(format!("    MOVWF 0x{r:02X}"));
        self.emit("    RETURN".to_string());
    }
}

/// The classic iterative dominator sets for a function's CFG: `doms[b]` is
/// the set of blocks that dominate block `b`. Used to classify the phi-copy
/// edges: `pred -> merge` is a BACK edge iff `merge` dominates `pred`: the
/// pred is inside the merge's loop, so on that edge the merge's phi slots
/// hold the CURRENT iteration's values. This covers self-loops
/// (pred == merge) AND separate-latch back edges (pred is a latch block).
fn block_dominators(f: &ir::Func) -> HashMap<String, HashSet<String>> {
    let entry = &f.blocks[0].label;
    let all: HashSet<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
    // Predecessor lists from the terminators' targets (the terminator is
    // the last inst of every block).
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    for b in &f.blocks {
        let targets: Vec<&str> = match b.insts.last() {
            Some(Inst::Br(br)) => vec![br.target.as_str()],
            Some(Inst::BrCond(bc)) => vec![bc.t.as_str(), bc.f.as_str()],
            _ => vec![],
        };
        for t in targets {
            preds.entry(t).or_default().push(b.label.as_str());
        }
    }
    let mut dom: HashMap<String, HashSet<String>> = HashMap::new();
    dom.insert(entry.clone(), HashSet::from([entry.clone()]));
    for b in &f.blocks {
        if b.label != *entry {
            dom.insert(b.label.clone(), all.clone());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            if b.label == *entry {
                continue;
            }
            let mut new: HashSet<String> = match preds.get(b.label.as_str()) {
                Some(ps) => ps
                    .iter()
                    .map(|p| dom[*p].clone())
                    .reduce(|a, c| a.intersection(&c).cloned().collect())
                    .unwrap_or_else(|| all.clone()),
                None => all.clone(),
            };
            new.insert(b.label.clone());
            if new != dom[&b.label] {
                dom.insert(b.label.clone(), new);
                changed = true;
            }
        }
    }
    dom
}

/// Emit one function's body into `g.out`: runtime routines get their recipe
/// body; ordinary functions get the block labels, phi copies, and
/// terminators. Shared by both emission passes: pass A measures the body
/// (every PCLATH restore present) to drive the page assignment, pass
/// B re-emits it with same-page restores skipped.
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    // Runtime routines (legalize-injected): the entry block holds only the
    // scratch alloca, so instead of the (empty) block emission the recipe
    // body goes here: the label, the adapted epicurus asm, and the RETURN
    // the injected Func has no `ret` for. A routine with no recipe yet
    // panics loudly rather than emitting an empty label that would silently
    // fall through into the next function.
    if let Some(recipe) = routine_recipe(&f.name) {
        match recipe {
            "__mul_u8" | "__mul_u16" | "__mul_u32" | "__udiv_u8" | "__urem_u8" | "__udiv_u16"
            | "__urem_u16" | "__udiv_u32" | "__urem_u32" | "__sdiv_i8" | "__srem_i8"
            | "__sdiv_i16" | "__srem_i16" | "__sdiv_i32" | "__srem_i32" | "__shl_u8"
            | "__lshr_u8" | "__ashr_i8" | "__shl_u16" | "__lshr_u16" | "__ashr_i16"
            | "__shl_u32" | "__lshr_u32" | "__ashr_i32" | "__add_f32" | "__sub_f32"
            | "__mul_f32" | "__div_f32" | "__cmp_f32" | "__uitofp_f32" | "__sitofp_f32"
            | "__fptoui_f32" | "__fptosi_f32" => {}
            other => panic!("isel: unknown runtime routine @{other}"),
        }
        g.emit_routine();
        return;
    }
    // CC-4 naked: verbatim, no prologue, panic on non-Asm, barrier markers.
    if f.naked {
        g.emit(format!("{}:", f.name));
        g.emit("; --- asm start ---".to_string());
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Asm(a) => {
                        let substituted = g.substitute_asm(&a.template, &a.operands);
                        for line in substituted.split('\n') {
                            g.emit(line.to_string());
                        }
                    }
                    _ => panic!(
                        "isel: naked function '{}' contains non-asm instruction; naked bodies must be pure assembly",
                        f.name
                    ),
                }
            }
        }
        g.emit("; --- asm end ---".to_string());
        g.emit("".to_string());
        return;
    }
    // Block label scheme: the entry block uses the bare function name
    // (so CALLs and GOTOs resolve to it); every other block is
    // `{func}_L{label}`. The entry block's label is emitted by the block
    // loop below: no standalone function label here, or `main:` /
    // `add:` would be defined twice and gpasm would reject the file.
    let mut labels: HashMap<String, String> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let lbl = if i == 0 {
            f.name.clone()
        } else {
            format!("{}_L{}", f.name, b.label)
        };
        labels.insert(b.label.clone(), lbl);
    }
    // phi elimination: for each (predecessor, merge) edge, the copies that
    // must run when that edge is taken. Keyed by the edge, NOT just the
    // predecessor: the copies must run ONLY on the edge to their merge
    // block: running them unconditionally clobbers the phi slots with
    // next-iteration values that the other branch's target reads (found by
    // the fuzz corpus: clang folds `acc = i` loops into cross-referencing
    // phis, and the exit block read the clobbered accumulator).
    let mut phi_copies: HashMap<(String, String), Vec<(String, Ty, Val)>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Phi(p) = i {
                for (val, pred) in &p.incoming {
                    phi_copies
                        .entry((pred.clone(), b.label.clone()))
                        .or_default()
                        .push((p.dst.clone(), p.ty, val.clone()));
                }
            }
        }
    }
    // Back-edge classifier for the phi-copy ordering: `merge` dominates the
    // pred (self-loop OR separate-latch back edge) => the merge's phi slots
    // hold the current iteration's values and readers run first.
    let doms = block_dominators(f);
    for (i, b) in f.blocks.iter().enumerate() {
        g.emit(format!("{}:", labels[&b.label]));
        if i == 0 && f.isr {
            // The ISR save prologue, right after the vector entry (word 4):
            // W into 0x75, nibble-swapped IN PLACE (SWAPF 0x75, F has no
            // STATUS side effects and no W dependency, the epilogue's
            // swap-back is the flag-safe W restore), STATUS into 0x76
            // nibble-swapped (SWAPF reads STATUS without touching it),
            // PCLATH and FSR into 0x77/0x78, then the preempted main's
            // in-flight return value (0x71-0x74) into 0x79-0x7C: the ISR
            // body's value-returning calls write the retval region, so
            // without this save they would clobber it, and finally the
            // fixed scratch byte 0x70 into 0x7D. The scratch is LIVE across
            // interrupt windows in the preempted main: const reads stash
            // their byte/index in 0x70 across the PCLATH restore, GEP
            // offsets accumulate there, and the icmp/add/sub chains fold
            // through it: an ISR that itself uses the scratch (a const
            // read, a compare, an i16/i32 op) would silently corrupt that
            // in-flight value without this save. Then PCLATH = 0 so the
            // ISR body's GOTOs stay in page 0 (the M11 restore literal is
            // PAGE(isr) = 0). The save area is fixed common RAM
            // (0x75-0x7D: W/STATUS/PCLATH/FSR/retval x4/scratch = 9 bytes),
            // disjoint from the scratch byte (0x70) and the retval region
            // (0x71-0x74); 0x7E-0x7F stays free. The retval/scratch MOVFs
            // clobber the CURRENT Z, which is harmless: the interrupted
            // STATUS is already safe in 0x76.
            g.emit("    MOVWF 0x75");
            g.emit("    SWAPF 0x75, F");
            g.emit("    SWAPF STATUS, W");
            g.emit("    MOVWF 0x76");
            g.emit("    MOVF PCLATH, W");
            g.emit("    MOVWF 0x77");
            g.emit("    MOVF FSR, W");
            g.emit("    MOVWF 0x78");
            g.emit("    MOVF 0x71, W");
            g.emit("    MOVWF 0x79");
            g.emit("    MOVF 0x72, W");
            g.emit("    MOVWF 0x7A");
            g.emit("    MOVF 0x73, W");
            g.emit("    MOVWF 0x7B");
            g.emit("    MOVF 0x74, W");
            g.emit("    MOVWF 0x7C");
            g.emit("    MOVF 0x70, W");
            g.emit("    MOVWF 0x7D");
            g.emit("    MOVLW 0x00");
            g.emit("    MOVWF PCLATH");
        }
        let mut terminator = None;
        for i in &b.insts {
            match i {
                Inst::Phi(_) => {} // eliminated; copies emitted at pred ends
                Inst::Br(_) | Inst::BrCond(_) | Inst::Ret(_) => terminator = Some(i),
                _ => g.emit_inst(i),
            }
        }
        if let Some(t) = terminator {
            match t {
                Inst::Br(br) => {
                    let merge = br.target.clone();
                    if let Some(c) = phi_copies.get(&(b.label.clone(), merge.clone())) {
                        g.emit(format!("    ; phi copies for pred {0}", labels[&b.label]));
                        emit_phi_copies(g, c, doms[&b.label].contains(&merge));
                    }
                    g.emit(format!("    GOTO {}", labels[&merge]));
                }
                Inst::BrCond(bc) => {
                    let lt = labels[&bc.t].clone();
                    let lf = labels[&bc.f].clone();
                    let t_copies = phi_copies.get(&(b.label.clone(), bc.t.clone()));
                    let f_copies = phi_copies.get(&(b.label.clone(), bc.f.clone()));
                    match &bc.cond {
                        Val::Reg(r) => {
                            let ca = g.val_addr(&Val::Reg(r.clone())).direct();
                            g.emit(format!("    MOVF 0x{ca:02X}, W"));
                            match (t_copies, f_copies) {
                                // Plain branch: the classic BTFSC skip shape.
                                (None, None) => {
                                    g.emit("    BTFSC STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                                // Both targets are merges: f falls through to
                                // its copies, t jumps to a copy block.
                                (Some(ct), Some(cf)) => {
                                    let lcop = g.fresh_label();
                                    g.emit("    BTFSS STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lcop}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, cf, doms[&b.label].contains(&bc.f));
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!("{lcop}:"));
                                    emit_phi_copies(g, ct, doms[&b.label].contains(&bc.t));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                                // The copies feed the f (cond==0 fall-through)
                                // edge: skip over them to t when cond != 0.
                                (_, Some(c)) => {
                                    g.emit("    BTFSS STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lt}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.f));
                                    g.emit(format!("    GOTO {lf}"));
                                }
                                // The copies feed the t (cond!=0 jump) edge:
                                // skip over f to them when cond == 0.
                                (Some(c), None) => {
                                    g.emit("    BTFSC STATUS, 2 ; Z".to_string());
                                    g.emit(format!("    GOTO {lf}"));
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.t));
                                    g.emit(format!("    GOTO {lt}"));
                                }
                            }
                        }
                        Val::Const(k) => {
                            if *k != 0 {
                                if let Some(c) = t_copies {
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.t));
                                }
                                g.emit(format!("    GOTO {lt}"));
                            } else {
                                if let Some(c) = f_copies {
                                    g.emit(format!(
                                        "    ; phi copies for pred {0}",
                                        labels[&b.label]
                                    ));
                                    emit_phi_copies(g, c, doms[&b.label].contains(&bc.f));
                                }
                                g.emit(format!("    GOTO {lf}"));
                            }
                        }
                        Val::Global(_) => panic!("isel: conditional branch on a global"),
                    }
                }
                _ if f.isr => {
                    match t {
                        // The restore epilogue replaces the ISR's `ret`. Order
                        // is load-bearing: the retval region (0x79-0x7C ->
                        // 0x71-0x74), then the scratch byte (0x7D -> 0x70), then
                        // PCLATH/FSR (MOVF, their Z clobbers are fine, STATUS
                        // is not yet restored), then STATUS via the nibble
                        // swap-back (SWAPF is flag-safe), and W LAST via its
                        // swap-back (also flag-safe, MOVF would set Z from the
                        // moved value after STATUS was already restored,
                        // corrupting the interrupted main's Z). RETFIE pops the
                        // hardware-pushed return.
                        Inst::Ret(None) => {
                            g.emit("    MOVF 0x79, W");
                            g.emit("    MOVWF 0x71");
                            g.emit("    MOVF 0x7A, W");
                            g.emit("    MOVWF 0x72");
                            g.emit("    MOVF 0x7B, W");
                            g.emit("    MOVWF 0x73");
                            g.emit("    MOVF 0x7C, W");
                            g.emit("    MOVWF 0x74");
                            g.emit("    MOVF 0x7D, W");
                            g.emit("    MOVWF 0x70");
                            g.emit("    MOVF 0x77, W");
                            g.emit("    MOVWF PCLATH");
                            g.emit("    MOVF 0x78, W");
                            g.emit("    MOVWF FSR");
                            g.emit("    SWAPF 0x76, W");
                            g.emit("    MOVWF STATUS");
                            g.emit("    SWAPF 0x75, W");
                            g.emit("    RETFIE");
                        }
                        Inst::Ret(Some(_)) => panic!(
                            "isel: interrupt handler @{} must be void (cannot return a value)",
                            f.name
                        ),
                        _ => unreachable!(),
                    }
                }
                _ => g.emit_terminator(t, &labels),
            }
        }
    }
    g.emit("".to_string());
}

/// Emit the dependency-ordered phi copies for one (pred -> merge) edge: a
/// copy never overwrites a slot a later copy still needs to read.
///
/// The ordering depends on whether the edge is a BACK edge into the merge
/// (`back_edge`, computed by `block_dominators`, the merge block dominates
/// the pred, so the pred is inside the merge's loop):
/// - Back edge (a self-loop OR a separate-latch back edge): the merge's phi
///   slots hold the CURRENT iteration's values, so a copy reading a slot
///   another copy writes must run BEFORE the overwrite (reader first). The
///   folded-induction loop `%acc <- %i, %i <- %i+1` needs acc before i, and
///   a two-block loop's cross-referencing phis (`%i <- %i.next,
///   %acc <- %i` on the latch edge) need the OLD i: writer-first emits
///   `%i <- %i.next` then `%acc <- %i`, so the accumulator reads the NEW
///   induction value: acc = n instead of n-1 (the same seed-75 off-by-one
///   class the fuzz corpus found on the self-loop form; the generated
///   for-loops are single-block, so the separate-latch edge went wrong
///   silently).
/// - Forward edge: a phi slot is only defined by THIS edge's copies, so a
///   copy reading a slot another copy writes must run AFTER its definer
///   (writer first; the %p <- %a, %q <- %p chain). This is why the
///   discriminator is the edge's CFG position (dominance), not merely
///   whether a copy's source is one of the merge's phi destinations: the
///   same slot-aliasing shape needs writer-first on a forward edge (the
///   source slot is not live yet) and reader-first on a back edge (it
///   holds the current iteration's value).
/// A true cycle (%a <- %b, %b <- %a, a loop-carried swap) needs a temp
/// register, so it panics loudly rather than silently miscompile.
fn emit_phi_copies<'m>(g: &mut Gen<'m>, copies: &[(String, Ty, Val)], back_edge: bool) {
    let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
        .iter()
        .map(|(dst, ty, val)| {
            let da = g.slot_addr(g.cur_func, dst).direct();
            let src = match val {
                Val::Reg(r) => {
                    if g.resolved.contains_key(&ssa_key(g.cur_func, r)) {
                        None
                    } else {
                        Some(g.slot_addr(g.cur_func, r).direct())
                    }
                }
                _ => None,
            };
            (da, src, *ty, val.clone())
        })
        .collect();
    let n = pending.len();
    let mut emitted = vec![false; n];
    let mut emitted_count = 0usize;
    while emitted_count < n {
        let mut progress = false;
        for i in 0..n {
            if emitted[i] {
                continue;
            }
            let (da, src, ty, val) = &pending[i];
            let blocked = if back_edge {
                // Reader first: blocked while an un-emitted sibling READS
                // this copy's destination (sibling source == my dst): that
                // sibling reads a merge phi slot holding the current
                // iteration's live value and must run before the overwrite.
                (0..n).any(|j| !emitted[j] && j != i && pending[j].1 == Some(*da))
            } else {
                // Writer first: blocked while an un-emitted sibling WRITES
                // this copy's source (sibling dst == my src): on a forward
                // edge the source slot is only defined by this edge's
                // copies, so a reader runs after its definer.
                match src {
                    Some(s) => (0..n).any(|j| !emitted[j] && j != i && pending[j].0 == *s),
                    None => false,
                }
            };
            if !blocked {
                g.emit_move_val_to_slot(val, *ty, *da);
                emitted[i] = true;
                emitted_count += 1;
                progress = true;
            }
        }
        if !progress {
            panic!("isel: cyclic phi copies not supported");
        }
    }
}

/// The word size of a function's emitted lines: 1 word per instruction line
/// (labels, `.align`/`.table` directives, `equ` lines, comments, and blanks
/// are 0), mirroring the asm crate's pass-1 counting so the page-fit
/// decisions match the addresses the assembler will assign.
fn word_size(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|raw| {
            let line = raw.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                return false;
            }
            if line.starts_with("list") || line.starts_with("radix") {
                return false;
            }
            if line.starts_with("org ") {
                return false;
            }
            if line.starts_with("end") {
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
        })
        .count()
}

/// Greedy page assignment (M11), one function: pad with `.org <next base>`
/// before a function that would cross the current 2048-word page's end, and
/// before ANY function whose start is page-aligned (an overflow continuation
/// or an exact-boundary one), and return the `(pad, page, next_addr)`. A
/// function larger than one page can never fit (its intra-function GOTOs
/// need a single stable page) and panics loudly; a program past page 3
/// (0x2000, the device flash) panics loudly too. The `.org` pads with
/// 0x0000 words (the assembler supports it), so the final layout's addresses
/// are exactly what the tracker predicts.
/// Verify the FINAL post-banking layout's page fit (issue #17): every
/// function must lie entirely inside one 2048-word page, or its label
/// resolves to the lower page while its later words sit in the upper one
/// and its intra-function GOTOs (`PAGE(<func>)` from the label) misbranch.
///
/// The bin-packing assignment (issue #12) runs on POST-banking sizes: the
/// banking pass's BANKSEL growth is measured before packing, so a function
/// that would straddle a boundary is packed into the next page with an
/// anchor instead. The `.org` pads pin page bases, so the elision cannot
/// move a function off its assigned page. This
/// check walks the final text (the exact layout the assembler will place)
/// with the same pass-1 semantics `asm::assemble` uses: `.org` jumps,
/// `.align N` pads to N-word boundaries, labels take no words, `equ`/
/// `.table`/`list`/`radix`/`end` emit none, tracks each function's actual
/// extent from its label to the next function's label (or the program end),
/// and panics loudly on any straddle or page overflow. `__start` and the
/// const-table reader entries are checked the same way (a reader is the
/// target of `PAGE(__read_<name>)` sets, so it must not straddle either).
///
/// The extent measure is conservative in the safe direction: the next
/// function's label is the FIRST word the current function cannot own, and
/// nothing in between (an `.org` pad, a `.align`) belongs to the function
/// whose label precedes it, so a function that ends exactly at a page
/// boundary (its last word is the boundary page's last word) passes, and
/// only a true straddle panics. The check runs on the post-peephole text
/// in the driver pipeline, so every pass that can move words is covered.
pub fn verify_page_fit(m: &Module, asm: &str) {
    let funcs: Vec<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    // `__read_<name>` / `__read_<name>_hi` reader entries: CALL targets of
    // `PAGE(__read_*)` sets, so each must lie inside one page too.
    let mut readers: Vec<String> = Vec::new();
    for g in &m.globals {
        if g.is_const {
            readers.push(format!("__read_{}", g.name));
            if g.bytes.len() >= 256 {
                let n_chunks = ((g.bytes.len() + 255) / 256).max(2);
                for c in 1..n_chunks {
                    readers.push(if c == 1 {
                        format!("__read_{}_hi", g.name)
                    } else {
                        format!("__read_{}_hi{c}", g.name)
                    });
                }
            }
        }
    }
    let mut org = 0usize;
    let mut cur: Option<(String, usize)> = None;
    let check = |name: &str, s: usize, e: usize| {
        if e <= s {
            return; // empty extent (a label with no words before the next target)
        }
        let last = e - 1;
        if last / 0x800 != s / 0x800 {
            panic!(
                "isel: post-banking page-fit failure: {name} spans pages (0x{s:04X}-0x{last:04X}): the banking pass grew it across a page boundary; its label resolves to page {} while its tail sits in page {}",
                s / 0x800,
                last / 0x800
            );
        }
    };
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            let target = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            if let Some((name, s)) = &cur {
                check(name, *s, target);
            }
            cur = None;
            org = target;
            continue;
        }
        if line.starts_with("end") {
            break;
        }
        if let Some(l) = line.strip_suffix(':') {
            let name = l.trim().to_string();
            let is_target =
                funcs.contains(&name.as_str()) || name == "__start" || readers.contains(&name);
            if is_target {
                if let Some((prev, s)) = &cur {
                    check(prev, *s, org);
                }
                cur = Some((name, org));
            }
            // Internal (block) labels keep the current target's extent open:
            // the words after them still belong to the function whose label
            // opened the extent.
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
            // The const-table DATA begins here. Table bytes may legitimately
            // straddle a page boundary (reads are 256-byte-window based, not
            // page based, the reader's computed jump reaches across), so the
            // reader entry's extent must END here, not include the table.
            if let Some((name, s)) = &cur {
                check(name, *s, org);
            }
            cur = None;
            continue;
        }
        org += 1;
    }
    if let Some((name, s)) = &cur {
        check(name, *s, org);
    }
}

/// The const-table section's reader entries and the page PCLATH<4:3> holds
/// after each CALL returns: a reader writes `MOVLW HIGH(<base>); MOVWF
/// PCLATH` before its computed jump, so PCLATH's page bits are the TABLE
/// BASE's page (`<name>` for single/chunk-0 reads, `<name>_1` for chunk-1
/// reads): not `PAGE(__read_<name>)`, the page the caller set (the entry
/// itself can sit in a different page than its base, e.g. a reader at
/// 0x7FA with a 256-aligned base at 0x800). The caller's restore after the
/// call is needed iff that page differs from its own, so this is the map
/// `emit_pclath_restore` consults. The section's pass-A placement is used:
/// it sits right after the last function, and pass B re-pins the section to
/// this exact start with a leading `.org` whenever the pass-B elision would
/// move a reader base across a page boundary (see the pass-B note in
/// `select`), so no reader base can drift across a page boundary between
/// the passes: the pages hold in the final text.
fn reader_pages(consts: &[&ir::Global], table_start: usize) -> Vec<(String, usize)> {
    let mut pages = Vec::new();
    let mut addr = table_start;
    for g in consts {
        let size = g.bytes.len();
        if size >= 256 {
            // Reader entry (6 words), `.align 256`, chunk 0 base at the
            // aligned address; chunks c >= 1 sit exactly +256c later, and
            // their reader entries are emitted AFTER the table (6 words
            // each, in chunk order).
            let n_chunks = ((size + 255) / 256).max(2);
            let aligned = ((addr + 6) + 255) & !255;
            pages.push((format!("__read_{}", g.name), aligned / 0x800));
            for c in 1..n_chunks {
                let entry = if c == 1 {
                    format!("__read_{}_hi", g.name)
                } else {
                    format!("__read_{}_hi{c}", g.name)
                };
                pages.push((entry, (aligned + 256 * c) / 0x800));
            }
            addr = aligned + 256 * (n_chunks - 1) + (size - 256) + 6 * (n_chunks - 1);
        } else {
            // Single table: base sits 6 words (the reader entry) after the
            // section's running address.
            pages.push((format!("__read_{}", g.name), (addr + 6) / 0x800));
            addr += 6 + size;
        }
    }
    pages
}

/// Select instructions for the whole module, producing PIC14 assembly text.
///
/// `addrs` is the complete address map from `alloc`: globals by name, locals
/// by `{func}::{name}` (IR value names without `%`). isel does no slot
/// allocation: every value's address is read from the map. The icmp scratch
/// byte and the four retval bytes live in fixed common RAM (scratch `0x70`,
/// retval `0x71`-`0x74`): bank-independent, never used by locals (M3), so no
/// BANKSEL is ever needed for them.
///
/// M11: every CALL runs with PCLATH<4:3> = the target's page (set
/// immediately before, restored immediately after, the restore is skipped
/// when the target is in the caller's own page), functions are assigned to
/// 2048-word pages by first-fit bin packing over their post-banking sizes
/// (a function that would cross a page's end gets a
/// `.org <next base>` pad), and the program's highest word address is
/// bounded by the device's 8K-word flash. Emission is two-phase: pass A
/// measures every body (all restores present), measures the post-banking
/// growth, and assigns pages for ALL
/// functions so a forward call target's page is known; pass B re-emits with
/// same-page restores skipped (the pads pin the page bases, so the elision
/// never moves a function off its assigned page).
pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    // The device's interrupt vector(s) (the hardware pushes the return PC
    // and clears GIE; PCLATH is untouched). The vector IS the ISR entry:
    // no GOTO, since a GOTO's target page would depend on the interrupted
    // PCLATH (unknowable), so the ISR is emitted FIRST with a `.org 4` pad
    // (words 2-3 after the reset entry), and `__start` moves after it. More
    // interrupt handlers than the device has vectors would fight over one
    // vector: panic loudly.
    let isr_names: Vec<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr)
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        isr_names.len() <= device.interrupt_vectors.len(),
        "isel: {} interrupt handlers ({}): {} has {} interrupt vector(s)",
        isr_names.len(),
        isr_names.join(", "),
        device.name,
        device.interrupt_vectors.len(),
    );
    let has_isr = !isr_names.is_empty();
    // The icmp scratch byte and the four retval bytes are fixed common-RAM
    // constants (bank-independent, the device's common RAM is never used by
    // locals, so no collision). The widened i32 region must not overrun
    // common RAM nor overlap the scratch byte, and the ISR save area (W,
    // STATUS, PCLATH, FSR, retval x4, scratch, 9 bytes) must sit right
    // after the retval region, disjoint from it and from scratch, leaving
    // the last 2 bytes of common RAM free.
    let (common_lo, common_hi) = device
        .common_ram
        .expect("isel's fixed scratch/retval/ISR-save layout needs a common-RAM region");
    let scratch: u16 = common_lo;
    let retval_lo: u16 = common_lo + 1;
    let isr_save_lo: u16 = common_lo + 5;
    let isr_save_hi: u16 = common_lo + 13;
    assert!(
        retval_lo + 4 <= common_hi + 1,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must fit in common RAM",
        retval_lo + 3
    );
    assert!(
        retval_lo + 4 <= isr_save_lo,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must not overlap the ISR save area 0x{isr_save_lo:02X}-0x{isr_save_hi:02X}",
        retval_lo + 3
    );
    assert!(
        isr_save_hi + 1 <= common_hi,
        "isel: ISR save area 0x{isr_save_lo:02X}-0x{isr_save_hi:02X} must leave 0x{:02X}-0x{common_hi:02X} free",
        isr_save_hi + 1,
    );
    let mut out: Vec<String> = Vec::new();
    out.extend(vec![
        "; pic8 -- integer spine milestone 2 (isel)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR    equ 0x04".to_string(),
        "INDF   equ 0x00".to_string(),
        "PCL    equ 0x02".to_string(),
        "PCLATH equ 0x0A".to_string(),
        "INTCON equ 0x0B".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ]);
    if !m.module_asm.is_empty() {
        out.push("; module asm".to_string());
        for entry in &m.module_asm {
            for line in entry.split('\n') {
                out.push(line.to_string());
            }
        }
    }
    if !has_isr {
        // No ISR: `__start` sits at the top (word 2) so the reset vector's
        // GOTO (PCLATH = 0 at reset) always reaches it, byte-identical to
        // the pre-interrupt layout. With an ISR the vector owns word 4, so
        // `__start` is emitted after the ISR body instead (see pass B).
        out.extend([
            "__start:".to_string(),
            "    MOVLW PAGE(main)".to_string(),
            "    MOVWF PCLATH".to_string(),
            "    CALL main".to_string(),
            "    SLEEP".to_string(),
            "".to_string(),
        ]);
    }
    // Phase-3 pointers: resolve every GEP's chain eagerly to a folded
    // `(base, k, terms)`, keyed `{func}::{reg}` like every other local.
    // Seeds first: a byval param slot IS the struct copy (Slot(name,
    // false)); an sret param slot holds the target address (Slot(name,
    // true)); an alloca defines its own buffer slot (Slot(name, false)).
    // Gep itself is virtual: it emits nothing. The fold (shared with
    // isel-pic18) lives in `iselcore::resolve_pointers`.
    let resolved = resolve_pointers(m);
    // Fresh-label counter at module scope: labels are file-scoped in the
    // single `.asm` output, so it must not reset per function.
    // ---- PASS A: emit every function body with every PCLATH restore
    // present, measure word sizes, and run the page assignment over
    // ALL functions. A single-pass emission cannot know a forward call
    // target's page while the caller is being emitted (the target's
    // placement depends on sizes measured later), so pass A measures and
    // assigns first; pass B (below) emits the final text with every
    // function's page known.
    // Emission order: the ISR first (it owns the vector at word 4), then
    // every other function in module order.
    let mut order: Vec<&ir::Func> = Vec::with_capacity(m.funcs.len());
    order.extend(m.funcs.iter().filter(|f| f.isr));
    order.extend(m.funcs.iter().filter(|f| !f.isr));
    let mut bodies: Vec<(String, usize)> = Vec::new();
    let mut body_texts: Vec<String> = Vec::new();
    {
        let mut tmp = 0u32;
        for f in &order {
            let mut g = Gen {
                m,
                addrs,
                device,
                resolved: &resolved,
                scratch,
                retval_lo,
                cur_func: &f.name,
                tmp: &mut tmp,
                page_of: None,
                out: Vec::new(),
            };
            emit_func_body(&mut g, f);
            bodies.push((f.name.clone(), word_size(&g.out)));
            body_texts.push(g.out.join("\n"));
        }
    }
    // The banking pass inserts BANKSEL words that grow the text (issue #17).
    // The bin packing must fit the FINAL post-banking sizes, or a function
    // packed into a tight page tail can straddle the boundary after banking
    // (the greedy layout had slack; first-fit's tighter packing does not).
    // Per-function BANKSEL counts are placement-independent: every label
    // resets the tracked bank, and callee exit banks are callee-local, so
    // measuring once on the pass-A text (all PCLATH restores present) is
    // exact, and pass B's same-page restore elision only shrinks bodies, so
    // the packed layout is elision-stable.
    let mut measure: Vec<String> = vec![
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    if !has_isr {
        measure.extend([
            "__start:".to_string(),
            "    MOVLW PAGE(main)".to_string(),
            "    MOVWF PCLATH".to_string(),
            "    CALL main".to_string(),
            "    SLEEP".to_string(),
            "".to_string(),
        ]);
    }
    for (i, (name, _)) in bodies.iter().enumerate() {
        measure.push(body_texts[i].clone());
        if has_isr && name == isr_names[0] {
            measure.extend([
                "__start:".to_string(),
                "    MOVLW PAGE(main)".to_string(),
                "    MOVWF PCLATH".to_string(),
                "    CALL main".to_string(),
                "    SLEEP".to_string(),
                "".to_string(),
            ]);
        }
    }
    let banked = banking::assign_banks(device, &measure.join("\n"));
    // Measure each function's post-banking extent (function label to the
    // NEXT function label, or the end) with the same pass-1 semantics
    // `asm::assemble` uses. Internal block labels keep the current
    // function's extent open: only function labels (and `__start`) close
    // it, exactly like `verify_page_fit`.
    let func_names: HashSet<&str> = order.iter().map(|f| f.name.as_str()).collect();
    let mut post: HashMap<String, usize> = HashMap::new();
    {
        let mut org = 0usize;
        let mut cur: Option<(String, usize)> = None;
        for raw in banked.lines() {
            let line = raw.split(';').next().unwrap_or("").trim();
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
    }
    // Bin-packing page assignment over every function's post-banking size,
    // in emission order, first-fit: each function goes to the LOWEST-numbered
    // page with room for it. The greedy next-fit only considered the current
    // page, so a page tail was wasted whenever the next function was even
    // slightly too large, even when a later small function could fill it;
    // first-fit reuses those tails (a small function later in the module
    // lands in an earlier page's tail, and the program uses fewer pages).
    // The running word address starts at 5 without an ISR: the reset vector
    // (1 word: `goto __start`) plus the `__start` body (4 words), with
    // `__start` at the top so the reset vector's GOTO (PCLATH = 0 at reset)
    // always reaches it. With an ISR the vector owns word 4: the ISR is
    // pinned there (no page pad, the vector IS the entry), `__start` moves
    // right after it, and the rest of the program follows. The ISR must fit
    // page 0 (0x004-0x7FF) AND leave `__start` inside page 0: the reset
    // GOTO runs with PCLATH = 0, so a `__start` at 0x800+ would be
    // unreachable; it panics loudly (ISRs are usually small).
    let mut pages: HashMap<String, usize> = HashMap::new();
    let mut pads: HashMap<String, usize> = HashMap::new();
    // `page_next[i]` = the next free word in page i (the running address of
    // the page's last placed function). Pages are opened in order, so the
    // last entry is the program's end: the const-table section's start.
    // Page 0 starts after the 5-word header (reset `goto __start` + the
    // 4-word `__start` body); with an ISR the vector owns word 4 and the
    // ISR branch below sets page 0's next free word after the ISR + `__start`.
    let mut page_next: Vec<usize> = vec![if has_isr { 4 } else { 5 }];
    for (name, _) in &bodies {
        let size = post[name];
        if has_isr && name == isr_names[0] {
            assert!(
                4 + size + 4 <= 0x800,
                "isel: isr @{name} of {size} words does not fit page 0 (0x004-0x7FF) with room for the reset __start"
            );
            pages.insert(name.clone(), 0);
            pads.insert(name.clone(), 4);
            // `size` is the ISR's post-banking body extent (label to the
            // `__start` label); the 4-word `__start` body follows it.
            page_next[0] = 4 + size + 4;
        } else {
            if size > 0x800 {
                panic!("isel: function @{name} of {size} words exceeds a 2048-word page (0x800)");
            }
            // First-fit: the lowest page whose tail fits this function.
            let mut placed: Option<(usize, usize)> = None;
            for (pi, next) in page_next.iter_mut().enumerate() {
                if *next + size <= (pi + 1) * 0x800 {
                    placed = Some((pi, *next));
                    *next += size;
                    break;
                }
            }
            let (page, start) = match placed {
                Some(p) => p,
                None => {
                    // No open page has room: open the next page. The device
                    // bound (flash_words) is enforced loudly.
                    let pi = page_next.len();
                    let last_page = device.flash_words / 0x800 - 1;
                    if pi as u32 >= device.flash_words / 0x800 {
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
            };
            pages.insert(name.clone(), page);
            // The anchor: a function whose start is page-aligned gets an
            // explicit `.org` pad: both the new-page case and the
            // exact-boundary continuation (the previous function's size
            // hit the boundary precisely, so the strict fit check alone
            // would emit no pad). Without it, pass B's same-page restore
            // elision shrinks the previous function and slides this one
            // below the boundary into a straddle: its label resolves to the
            // LOWER page while its later words sit in the upper one, so
            // intra-function GOTOs (PAGE(<func>) from the label) misbranch.
            if start & 0x7FF == 0 {
                pads.insert(name.clone(), start);
            }
        }
    }
    // Const-table readers: the page PCLATH holds after each `__read_*` CALL
    // (see `reader_pages`). The section sits right after the last function:
    // pass B pins it to this same start, so the pages hold in the final text.
    let mut consts: Vec<&ir::Global> = m.globals.iter().filter(|g| g.is_const).collect();
    consts.sort_by_key(|g| g.name.clone());
    let table_start = page_next
        .last()
        .copied()
        .unwrap_or(if has_isr { 4 } else { 5 });
    for (entry, page) in reader_pages(&consts, table_start) {
        pages.insert(entry, page);
    }
    // ---- PASS B: emit the final text with every function's page known.
    // Same-page calls (and same-page const reads) skip the restore pair; the
    // pages are the assignment's, and the `.org` pads pin the page bases, so
    // the elision cannot move a function off its assigned page (it only
    // shrinks bodies, page-membership-stable).
    //
    // Emission is in PAGE order, not module order: bin packing can place a
    // later function in an earlier page's tail, so module-order emission
    // would emit a backward `.org` (a page-0 function after a page-1 one):
    // the assembler panics on backward `.org`. Within a page, functions keep
    // their emission order (the page's running address is monotonic).
    let mut page_order: Vec<Vec<(&ir::Func, &str)>> = Vec::new();
    for (f, (name, _)) in order.iter().zip(&bodies) {
        let page = pages[name];
        while page_order.len() <= page {
            page_order.push(Vec::new());
        }
        page_order[page].push((*f, name.as_str()));
    }
    {
        let mut tmp = 0u32;
        let mut addr_b: usize = if has_isr { 4 } else { 5 };
        for funcs_on_page in &page_order {
            for (f, name) in funcs_on_page {
                let mut g = Gen {
                    m,
                    addrs,
                    device,
                    resolved: &resolved,
                    scratch,
                    retval_lo,
                    cur_func: &f.name,
                    tmp: &mut tmp,
                    page_of: Some(&pages),
                    out: Vec::new(),
                };
                emit_func_body(&mut g, f);
                if let Some(pad) = pads.get(*name) {
                    out.push(format!("    org 0x{pad:04X}"));
                    addr_b = *pad;
                }
                addr_b += word_size(&g.out);
                out.extend(g.out);
                if f.isr {
                    // `__start` moves after the ISR (the vector owns word 4):
                    // the reset GOTO at word 0 still reaches it, since it stays in
                    // page 0 per the ISR fit check above.
                    out.extend([
                        "__start:".to_string(),
                        "    MOVLW PAGE(main)".to_string(),
                        "    MOVWF PCLATH".to_string(),
                        "    CALL main".to_string(),
                        "    SLEEP".to_string(),
                        "".to_string(),
                    ]);
                    addr_b += 4;
                }
            }
        }
        // Pin the const-table section to its pass-A `table_start` whenever
        // the pass-B elision would move a reader base across a page
        // boundary. `reader_pages` maps every reader entry's page from the
        // pass-A position, but pass B emits the tables at the post-elision
        // position (bodies only shrink, so the section shifts earlier): a
        // chunked table's `.align 256` can then round a base across a page
        // boundary (a base pass A aligned to exactly k*0x800 re-aligns into
        // page k-1 after a 2-word elision), silently invalidating the
        // restore-skip map: a caller that skipped its restore on the mapped
        // page is left with the reader's HIGH(<base>) page, the drifted one,
        // and its next GOTO misbranches. The `.org` re-pins the section so
        // the final addresses are exactly the pass-A ones and the map stays
        // exact, but only when a base's page actually changes (the common
        // case, a small drift that stays within the mapped page, needs no
        // pin). It is always forward (or equal): pass-B bodies are no
        // larger, so `addr_b <= table_start`. A module without consts has no
        // section to pin.
        if !consts.is_empty() {
            let pages_a = reader_pages(&consts, table_start);
            let pages_b = reader_pages(&consts, addr_b);
            let drift = pages_a
                .iter()
                .zip(&pages_b)
                .any(|((_, pa), (_, pb))| pa != pb);
            if drift {
                out.push(format!("    org 0x{table_start:04X}"));
            }
        }
    }
    // Const (flash) globals become RETLW tables, emitted after the
    // functions so the CALLs above resolve. Every `__read_<name>` reader
    // sets PCLATH = HIGH(<name>) first: the computed `ADDLW LOW(<name>);
    // MOVWF PCL` jump lands at PCLATH:PCL, so a table in a nonzero 256-byte
    // window needs the window set (the M5 reader left PCLATH stale, the
    // latent window bug). A table of 256+ bytes is emitted as two 256-byte
    // chunks: chunk 0's 256 RETLWs at the base label `<name>` (`.align 256`
    // pads it to a 256-word boundary so LOW(<name>) == 0), then chunk 1's
    // RETLWs at the fresh label `<name>_1` IMMEDIATELY after: `<name>` +
    // 256 in the address space, so LOW(<name>_1) == 0 too and the true
    // bound is 511 bytes (a table of exactly 256 bytes has an empty chunk
    // 1, unreachable since its valid indices are 0..255), then the
    // `__read_<name>_hi` entry AFTER the
    // table (its computed-goto jumps into the table; the entry instructions
    // are dead after MOVWF PCL). A `.table <name> <size>` directive is
    // emitted immediately before every table's base label; the assembler
    // enforces the window fit loudly (LOW + size <= 0x100 for single-entry
    // tables, LOW == 0 for chunked bases): a table that crosses its window
    // or a misaligned chunk base would silently misread, so it must fail
    // assembly, not miscompile. Tables beyond 511 bytes (three chunks)
    // panic loudly: out of scope.
    // Label-collision guard: every label a table emits, its base label, its
    // reader entry, and for chunked tables the fresh `{name}_1` chunk label
    // and `__read_{name}_hi` entry, must be unique across all consts. A
    // user `const t_1` (or `const __read_t_hi`) next to a chunked `const t`
    // would emit a duplicate label the assembler's symbol insert silently
    // overwrites (wrong reads, no error): panic loudly instead.
    {
        let mut labels: HashMap<String, String> = HashMap::new();
        for g in &consts {
            let mut claim = |label: String, what: String| {
                if let Some(prev) = labels.insert(label.clone(), what.clone()) {
                    panic!(
                        "isel: const-table label collision: `{label}` is both {prev} and {what}"
                    );
                }
            };
            claim(
                format!("__read_{}", g.name),
                format!("reader entry of const {}", g.name),
            );
            claim(g.name.clone(), format!("base label of const {}", g.name));
            // Chunk count matches the emitter: a 256-byte table still emits
            // the M10 empty chunk 1 + `_hi` reader.
            let n_chunks = if g.bytes.len() >= 256 {
                ((g.bytes.len() + 255) / 256).max(2)
            } else {
                1
            };
            for c in 1..n_chunks {
                claim(
                    if c == 1 {
                        format!("{}_1", g.name)
                    } else {
                        format!("{}_{}", g.name, c)
                    },
                    format!("chunk-{c} label of const {}", g.name),
                );
                claim(
                    if c == 1 {
                        format!("__read_{}_hi", g.name)
                    } else {
                        format!("__read_{}_hi{}", g.name, c)
                    },
                    format!("chunk-{c} reader entry of const {}", g.name),
                );
            }
        }
    }
    let mut addr = table_start;
    for g in consts {
        assert!(
            !g.bytes.is_empty(),
            "isel: const @{} has no table bytes",
            g.name
        );
        let size = g.bytes.len();
        // Chunks emitted: 256-byte tables keep the M10 empty chunk-1 +
        // `_hi` reader (the dispatch's bit-0 test references them, and the
        // old layout is documented); larger tables get ceil(size/256)
        // chunks.
        let n_chunks = if size >= 256 {
            ((size + 255) / 256).max(2)
        } else {
            1
        };
        assert!(
            size <= 65535,
            "isel: const @{} table of {size} bytes exceeds the 65535-byte 16-bit index bound",
            g.name
        );
        // `MOVLW HIGH` clobbers W, so the incoming index (W = byte index)
        // is stashed in the fixed scratch byte (0x70, free at a const
        // read) across the PCLATH set, then reloaded for the computed jump.
        let reader = |out: &mut Vec<String>, base: &str| {
            out.push(format!("    MOVWF 0x{:02X}", scratch));
            out.push(format!("    MOVLW HIGH({base})"));
            out.push("    MOVWF PCLATH".to_string());
            out.push(format!("    MOVF 0x{:02X}, W", scratch));
            out.push(format!("    ADDLW LOW({base})"));
            out.push("    MOVWF PCL".to_string());
        };
        if size >= 256 {
            // Chunked table: chunk 0's reader, then `.align 256` (the
            // assembler pads to the next 256-word boundary, so LOW(name) ==
            // 0), then the `.table` directive, then each chunk's RETLWs at
            // `name` (chunk 0), `name_1` (chunk 1), `name_2`, ...: every
            // chunk base is exactly 256 words after the previous, so every
            // LOW() == 0. The reader entries come AFTER the table: chunk
            // c's reader at `__read_<name>_hi[c]` (chunk 1 keeps the M10
            // `_hi` name for fixture stability). (The entries' computed
            // gotos jump into the table; the entry instructions are dead
            // after MOVWF PCL, so their placement cannot shift the chunks.)
            // A table of exactly 256 bytes gets this branch too (size >=
            // 256): chunk 1 is empty (`name_1:` with no RETLWs, its reader
            // immediately after) and unreachable: every valid index
            // 0..255 selects chunk 0.
            out.push(format!("__read_{}:", g.name));
            reader(&mut out, &g.name);
            out.push("    .align 256".to_string());
            out.push(format!("    .table {} {size}", g.name));
            out.push(format!("{}:", g.name));
            for b in &g.bytes[..256] {
                out.push(format!("    RETLW 0x{b:02X}"));
            }
            for c in 1..n_chunks {
                let start = c * 256;
                let end = (c + 1) * 256;
                let chunk_label = if c == 1 {
                    format!("{}_1", g.name)
                } else {
                    format!("{}_{}", g.name, c)
                };
                out.push(format!("{chunk_label}:"));
                for b in &g.bytes[start..end.min(size)] {
                    out.push(format!("    RETLW 0x{b:02X}"));
                }
            }
            // reader entries after the table
            for c in 1..n_chunks {
                let chunk_label = if c == 1 {
                    format!("{}_1", g.name)
                } else {
                    format!("{}_{}", g.name, c)
                };
                let entry = if c == 1 {
                    format!("__read_{}_hi", g.name)
                } else {
                    format!("__read_{}_hi{c}", g.name)
                };
                out.push(format!("{entry}:"));
                reader(&mut out, &chunk_label);
            }
        } else {
            // Single-entry table (<= 255 bytes): `.table` immediately before
            // the base label; the assembler asserts LOW(name) + size <= 0x100.
            out.push(format!("__read_{}:", g.name));
            reader(&mut out, &g.name);
            out.push(format!("    .table {} {size}", g.name));
            out.push(format!("{}:", g.name));
            for b in &g.bytes[..size] {
                out.push(format!("    RETLW 0x{b:02X}"));
            }
        }
        // Track the tables' `.align`/RETLW words so the running address
        // stays consistent (tables are unconstrained, their addresses
        // don't affect function placement, which is already decided).
        addr += 6; // reader entry (MOVWF/MOVLW/MOVWF/MOVF/ADDLW/MOVWF PCL)
        if size >= 256 {
            addr = (addr + 255) & !255; // `.align 256`
            addr += 256; // chunk 0 RETLWs
            addr += size - 256; // chunks 1.. RETLWs
            addr += 6 * (n_chunks - 1); // chunk reader entries
        } else {
            addr += size; // single-entry RETLWs
        }
        out.push("".to_string());
    }
    out.push("    end".to_string());
    out.join("\n")
}

/// `parse_map` lives in `iselcore` now (moved there per the P2 plan's
/// final-review fix notes: it is a plain text-format parser over `alloc`'s
/// output with nothing PIC14-specific about it, so `isel-pic18` should not
/// need a hard dependency on `isel` just to reach it). Re-exported here so
/// `isel`'s own binary (`src/bin/isel.rs`) keeps working unchanged.
pub use iselcore::parse_map;
