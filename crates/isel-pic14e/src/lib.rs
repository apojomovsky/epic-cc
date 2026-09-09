//! `isel-pic14e`: instruction selection for the Enhanced Mid-range
//! integer spine.
//!
//! The classic ISA families (byte, bit, literal) match classic PIC14
//! (docs/33 §D-1), so this forks the classic emitter mnemonic for
//! mnemonic. The integer spine covers `add`/`sub`/`and`/`or`/`xor`
//! (i8/i16), the ten `icmp` predicates, `call`/`ret`, and `phi`
//! elimination. The shared banking pass banks output via `MOVLB`/`BSR`.
//!
//! The divergent FSR machinery (16-bit FSR0/FSR1, `MOVIW`/`MOVWI`,
//! linear addressing) lives in the pointer section below. PIC14E keeps
//! one 0x0004 vector with hardware context save (docs/33 §D-4).
//! `verify_page_fit` keeps the 2K-word page model: PCLATH is 7 bits
//! and `MOVLP` loads it in one instruction.
//!
//! Pointer lowering resolves each GEP chain eagerly to a
//! `(Base, k, terms)` triple at each `load`/`store`/`memcpy` use.
//! `Base::Global` covers RAM and const flash. `Base::Slot` covers a
//! byval copy, an alloca, or an sret slot holding a target address.
//! `gep` and `alloca` emit nothing. The slot size comes from alloc.
//!
//! A constant offset accesses the plain file register. Dynamic terms
//! set FSR to `base + k` plus scaled registers. One scale-1 term keeps
//! the fast single-register offset shape. General sums accumulate in
//! the fixed scratch byte. An indirect base reads its target from the
//! slot contents.
//!
//! Pointers into const flash load via `CALL __read_<name>` from a RETLW
//! table after the functions. A store through a const base panics:
//! ROM is not writable. `memcpy` lowers to a byte loop over the same
//! machinery.
//!
//! Static FSR bases reach all four GPR banks via the IRP bit. Each FSR
//! setup writes `STATUS, 7` from base bit 8 first, then loads the low
//! byte. An accessed object fits one GPR window, or emission panics:
//! crossing an SFR hole mis-addresses.
//!
//! An indirect base sets IRP from the stored address. A runtime SFR
//! address uses the same path with no static BANKSEL: `INDF` reaches
//! the file space through FSR plus IRP (epic-cc#117).
//!
//! Every address comes from the caller map: globals by name, locals by
//! `{func}::{name}`. isel allocates no slots. A missing value panics:
//! the map owns layout.

use device::Device;
use ir::{BinOp, Inst, MemLen, Module, SrcLoc, Ty, Val};
use iselcore::{resolve_pointers, ssa_key, Base, Slot};
use std::collections::{HashMap, HashSet};

/// Returns the recipe for a runtime routine name, or `None` for other names.
/// Shares the routine set with `alloc` for bank rounding. An injected
/// routine holds only a scratch alloca, so emitting it directly leaves an
/// empty label that falls into the next function. A name with no recipe
/// panics: the recipe set is closed.
///
/// An interrupt-context copy shares the base recipe under its own label
/// and slots, so the ISR frame stays disjoint from main. A duplicated
/// user function takes the ordinary block path.
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

/// Reports whether an object crosses a GPR bank boundary (docs/33 §D-2).
/// PIC14E routes such objects through the linear region so one FSR spans
/// banks. Other cores lack this shape, and the allocator avoids it there.
fn object_straddles(device: &Device, base_addr: u16, span: u16) -> bool {
    let (_, end) = device
        .region_for(base_addr)
        .expect("isel: FSR base 0x{base_addr:03X} outside GPR space (device {device.name})");
    base_addr + span - 1 > end
}

/// Returns the FSR base for an object: the physical address inside one
/// bank, else the linear alias for a straddling object (docs/33 §D-2).
/// The alias adds the bank stride to the within-bank offset. PIC14E
/// loads the 16-bit base into FSR0L and FSR0H, so one FSR spans banks.
fn fsr_base(device: &Device, base_addr: u16, span: u16) -> u16 {
    if !object_straddles(device, base_addr, span) {
        return base_addr;
    }
    let bank = device
        .ram_banks
        .iter()
        .position(|&(s, e)| base_addr >= s && base_addr <= e)
        .expect("isel: FSR base 0x{base_addr:03X} outside GPR space (device {device.name})");
    // `physical_offset` is the within-bank offset (0x20-0x6F), i.e. the low
    // 7 bits of the physical address; the linear alias is
    // `0x2000 + bank*80 + (physical_offset - 0x20)`.
    0x2000 + (bank as u16) * 80 + ((base_addr & 0x7F) - 0x20)
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
    /// Page map for two-phase emission: each CALL target and const reader
    /// maps to the page PCLATH holds after return. `None` in pass A emits
    /// every restore for measurement. `Some` in pass B skips same-page
    /// restores.
    page_of: Option<&'m HashMap<String, usize>>,
    /// Tracks the slot address whose value also sits in W, or `None`.
    /// Collapses a store followed by a reload of the same slot (epic-cc#214).
    /// Wires only private result slots, never the address read or written,
    /// so volatile and interrupt-shared sources stay sound (epic-cc#217).
    /// Plain `emit` clears the cache, so staleness cannot cross other
    /// emission. Slots stay function-private under ISR duplication, so no
    /// outside sequence observes the cached value.
    w_holds: Option<u16>,
    /// The source location of the instruction currently being emitted, or
    /// `None` for compiler-generated glue (prologue, `__start`, const init,
    /// runtime routines). `emit` records it on the line it pushes, so the
    /// parallel `locs` vector stays index-aligned with `out`.
    cur_loc: Option<SrcLoc>,
    out: Vec<String>,
    /// One source location per emitted line, index-aligned with `out`.
    /// `None` marks a compiler-generated line (no source instruction).
    locs: Vec<Option<SrcLoc>>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.w_holds = None;
        self.out.push(s.into());
        self.locs.push(self.cur_loc.clone());
    }

    /// `MOVWF addr`, unless `addr` is already known to hold W's value from
    /// an immediately preceding `emit_w_store`/`emit_w_load` of the same
    /// address (a genuine no-op then: the byte there already equals W).
    /// Marks `addr` as holding W's value either way.
    fn emit_w_store(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            self.emit(format!("    MOVWF 0x{addr:02X}"));
        }
        self.w_holds = Some(addr);
    }

    /// Emits `MOVF addr, W` unless W already holds the address value.
    /// Marks the address as holding W either way, so repeats collapse.
    /// Widening this cache needs the same private-slot rule (epic-cc#214).
    fn emit_w_load(&mut self, addr: u16) {
        if self.w_holds != Some(addr) {
            self.emit(format!("    MOVF 0x{addr:02X}, W"));
        }
        self.w_holds = Some(addr);
    }

    /// Emits the restore pair after a CALL. Skips it when caller and
    /// target share a page: the set already wrote that page and callees
    /// preserve it. Pass A always emits for measurement. Pass B skips
    /// shrink-only elision pinned by `.org` pads.
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

    /// Resolves `{func}::{name}` to its base byte address from the caller
    /// map. A missing value panics: the map owns layout. Reports whether
    /// the name is a plain pointer param holding a runtime address.
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

    /// The byte width of function `func`'s va region: the maximum over its
    /// call sites of the sum of extra-arg widths (the args past the named
    /// params). Mirrors `alloc::va_sizes` exactly, so the boundary assert
    /// in `VaArg` sees the same size the allocator reserved.
    fn func_va_size(&self, func: &str) -> u16 {
        self.m
            .funcs
            .iter()
            .find_map(|f| {
                if f.name != func {
                    return None;
                }
                let named = f.params.len();
                let mut max_w: u16 = 0;
                for caller in &self.m.funcs {
                    for b in &caller.blocks {
                        for inst in &b.insts {
                            if let ir::Inst::Call(c) = inst {
                                if !c.callees.is_empty() || c.func != func {
                                    continue;
                                }
                                if c.args.len() <= named {
                                    continue;
                                }
                                let extra: u16 = c.args[named..]
                                    .iter()
                                    .map(|a| a.ty.map(|t| u16::from(t.bytes())).unwrap_or(2))
                                    .sum();
                                max_w = max_w.max(extra);
                            }
                        }
                    }
                }
                Some(max_w)
            })
            .unwrap_or(0)
    }

    /// Substitutes `$0`/`%0` placeholders with allocated operand addresses.
    /// Accepts direct globals and locals. Rejects GEP-derived pointers:
    /// only direct locals name a stable slot (docs/33 §D-3).
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
                // Masks to the byte: i8 constants print mod 256 as signed.
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

    /// Returns the runtime pointer value for a global at offset `k`.
    /// Uses the linear alias for straddling objects (docs/33 §D-2), else
    /// the physical address. The linear base keeps later FSR derefs clear
    /// of the common-RAM hole.
    fn ptr_value_addr(&self, name: &str, k: u8) -> u16 {
        let addr = self.global_addr(name);
        let span = self.global_size(name);
        if object_straddles(self.device, addr, span) {
            fsr_base(self.device, addr, span) + u16::from(k)
        } else {
            addr + u16::from(k)
        }
    }

    /// Whether `name` is a const (flash) global: read via RETLW tables.
    /// A const that was copied to RAM (alloc placed it in `addrs` because it
    /// is used as a pointer call argument) is treated as RAM.
    fn global_is_const(&self, name: &str) -> bool {
        if self.addrs.contains_key(name) {
            return false;
        }
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .is_const
    }
    /// Reports whether `name` is a function rather than a global. A function
    /// address stays a link-time LOW and HIGH literal, never a map lookup
    /// (epic-cc#73).
    fn is_function(&self, name: &str) -> bool {
        self.m.funcs.iter().any(|f| f.name == name)
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
                    Inst::IntToPtr(p) if p.dst == name => p.to.bytes(),
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

    /// Returns the byte span of a resolved FSR base: the whole object a
    /// pointer into it can touch. Globals span their size. Slots span the
    /// param width or alloca size. A missing object panics: every lowered
    /// pointer names a known object.
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

    /// Returns the folded `(base, k, terms)` for pointer reg `%r`. A name
    /// outside the resolved map panics: lowering covers every live pointer.
    fn resolved_for(&self, r: &str) -> (Base, u8, Vec<(u8, String)>) {
        let key = ssa_key(self.cur_func, r);
        self.resolved
            .get(&key)
            .cloned()
            .unwrap_or_else(|| panic!("isel: no gep for pointer %{r} ({key})"))
    }

    /// Completes a byte access at `ptr + byte_off`. `Direct` uses the file
    /// register. `Indirect` uses FSR through INDF. Emits setup for dynamic
    /// and indirect pointers. Const bases take the RETLW path for loads
    /// and panic for stores.
    fn emit_ptr_setup(&mut self, ptr: &Val, byte_off: u8) -> Addr {
        match ptr {
            Val::Global(g) => {
                assert!(
                    !self.global_is_const(g),
                    "isel: store to const (flash) global @{g}"
                );
                let span = self.global_size(g);
                if object_straddles(self.device, self.global_addr(g), span) {
                    // Routes a straddling global through FSR0 with the linear
                    // base, so one FSR spans banks (docs/33 §D-2). A direct
                    // access would cross into the common-RAM hole.
                    self.emit_fsr_to(self.global_addr(g), 0, &[], byte_off, span);
                    Addr::Indirect
                } else {
                    Addr::Direct(self.global_addr(g) + u16::from(byte_off))
                }
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        assert!(
                            !self.global_is_const(name),
                            "isel: store to const (flash) global @{name}"
                        );
                        let span = self.object_span(&base);
                        if terms.is_empty()
                            && !object_straddles(self.device, self.global_addr(name), span)
                        {
                            // Constant offset only, object fits one bank: the
                            // address is statically known, a plain
                            // file-register access, no FSR.
                            Addr::Direct(
                                self.global_addr(name) + u16::from(k) + u16::from(byte_off),
                            )
                        } else {
                            // Uses FSR0 with the linear base for dynamic terms
                            // or straddling objects (docs/33 §D-2), so one FSR
                            // spans banks.
                            self.emit_fsr_to(self.global_addr(name), k, &terms, byte_off, span);
                            Addr::Indirect
                        }
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        // Reads a plain pointer param like an sret slot: the
                        // slot holds an address, while a byval slot is data.
                        if *indirect || self.param_holds_addr(sname) {
                            self.emit_fsr_indirect(sa, k, &terms, byte_off);
                            Addr::Indirect
                        } else {
                            let span = self.object_span(&base);
                            if terms.is_empty() && !object_straddles(self.device, sa, span) {
                                Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
                            } else {
                                self.emit_fsr_to(sa, k, &terms, byte_off, span);
                                Addr::Indirect
                            }
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// Mirrors the addressing decision without emitting. Reports whether
    /// the access needs FSR0. Lets constant memcpy park the byte when the
    /// destination setup clobbers W.
    fn ptr_setup_is_indirect(&self, ptr: &Val, _byte_off: u8) -> bool {
        match ptr {
            Val::Global(g) => {
                let span = self.global_size(g);
                object_straddles(self.device, self.global_addr(g), span)
            }
            Val::Reg(r) => {
                let (base, _k, terms) = self.resolved_for(r);
                match &base {
                    Base::Global(name) => {
                        let span = self.object_span(&base);
                        !(terms.is_empty()
                            && !object_straddles(self.device, self.global_addr(name), span))
                    }
                    Base::Slot(sname, indirect) => {
                        let sa = self.slot_addr(self.cur_func, sname).direct();
                        if *indirect || self.param_holds_addr(sname) {
                            true
                        } else {
                            let span = self.object_span(&base);
                            !(terms.is_empty() && !object_straddles(self.device, sa, span))
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// Loads one byte of a pointer load or memcpy source into W. Direct
    /// bases read the file register. Dynamic bases set FSR first. Const
    /// bases call the RETLW reader. Large tables take the 16-bit index
    /// path with chunk dispatch.
    fn emit_ptr_load_byte(&mut self, ptr: &Val, byte_off: u8) {
        match ptr {
            Val::Reg(r) => {
                if let (Base::Global(name), k, terms) = self.resolved_for(r) {
                    if self.global_is_const(&name) {
                        if self.global_size(&name) > 255 {
                            // Reads a large table via the chunked entry: W
                            // holds the in-chunk index.
                            self.emit_const_read_large(&name, k, &terms, byte_off);
                        } else {
                            // Reads a RETLW table: sets PCLATH first since the
                            // set clobbers W, computes the index into W, calls
                            // the reader, then parks the byte across the
                            // restore before reloading it.
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
                    // Reads a const global as a pointer via the RETLW reader.
                    // Supports only small tables: large tables need chunk
                    // selection with a register index. A large table here
                    // panics: the shape is closed.
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
            Addr::Indirect => self.emit("    MOVF INDF0, W".to_string()),
        }
    }

    /// Stores one value byte to `RAM[ptr + byte_off]`. Sets up the address
    /// first since FSR computation clobbers W, then loads the value.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        // Sets up the address first: FSR computation clobbers W.
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => {
                self.emit_load_byte(val, byte_off);
                self.emit(format!("    MOVWF 0x{a:02X}"));
            }
            Addr::Indirect => {
                self.emit_load_byte(val, byte_off);
                self.emit("    MOVWF INDF0".to_string());
            }
        }
    }

    /// Copies `len` bytes between pointers with a runtime length
    /// (epic-cc#4). Consumes the 16-bit length reg. Keeps all loop state
    /// in fixed common RAM, so banking inserts no words inside skip pairs.
    /// Decrements the countdown with borrow-accurate wraps and tests zero
    /// at the top. Recomputes FSR per byte. Rejects const flash sources:
    /// flash reads need the index in W (epic-cc#4).
    fn emit_memcpy_dynamic(&mut self, dst: &Val, src: &Val, len: &Val) {
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        let cnt_lo: u16 = self.retval_lo; // 0x71, dead at a memcpy
        let cnt_hi: u16 = self.retval_lo + 1; // 0x72
        let idx: u16 = 0x7E; // documented free common byte
        let hold: u16 = 0x7F; // documented free common byte
                              // Recomputes FSR per byte: one FSR serves both
                              // pointers on this core.
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
            g.emit("    ADDWF FSR0L, F".to_string());
            g.emit("    BTFSC STATUS, 0".to_string());
            g.emit("    INCF FSR0H, F".to_string());
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
        // Tests zero in common RAM, so no BANKSEL splits the skip pair.
        self.emit(format!("    MOVF 0x{cnt_lo:02X}, W"));
        self.emit(format!("    IORWF 0x{cnt_hi:02X}, W"));
        self.emit("    BTFSC STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_done}"));
        // src[i] -> hold.
        emit_byte_fsr(self, src);
        self.emit("    MOVF INDF0, W".to_string());
        self.emit(format!("    MOVWF 0x{hold:02X}"));
        // dst[i] = hold.
        emit_byte_fsr(self, dst);
        self.emit(format!("    MOVF 0x{hold:02X}, W"));
        self.emit("    MOVWF INDF0".to_string());
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

    /// Sets FSR0 to `base + k + offset + scaled terms` for a spanned object.
    /// Loads the physical or linear base (docs/33 §D-2) into FSR0L and H,
    /// then adds the offset with carry. One scale-1 term keeps the fast
    /// register shape. General sums accumulate in scratch first. The offset
    /// stays in one byte, so one carry suffices.
    fn emit_fsr_to(
        &mut self,
        base_addr: u16,
        k: u8,
        terms: &[(u8, String)],
        byte_off: u8,
        span: u16,
    ) {
        let base = fsr_base(self.device, base_addr, span);
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: FSR offset k {k} + off {byte_off} out of byte range"
        );
        // FSR0 = base (16-bit).
        self.emit(format!("    MOVLW 0x{:02X}", (base & 0xFF) as u8));
        self.emit("    MOVWF FSR0L".to_string());
        self.emit(format!("    MOVLW 0x{:02X}", ((base >> 8) & 0xFF) as u8));
        self.emit("    MOVWF FSR0H".to_string());
        // W = k + byte_off + Σ terms (the offset, 8-bit).
        match terms {
            [] => {
                if kk != 0 {
                    self.emit(format!("    MOVLW 0x{kk:02X}"));
                } else {
                    self.emit("    MOVLW 0x00".to_string());
                }
            }
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
                if kk != 0 {
                    self.emit(format!("    ADDLW 0x{kk:02X}"));
                }
            }
        }
        // FSR0 += W with carry into FSR0H.
        self.emit("    ADDWF FSR0L, F".to_string());
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit("    INCF FSR0H, F".to_string());
    }

    /// Sets FSR0 from an indirect slot plus offset and terms. Loads both
    /// address bytes from the slot, then adds the offset with carry. Uses
    /// the physical address: sret targets fit one bank by construction.
    fn emit_fsr_indirect(&mut self, slot_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: indirect offset k {k} + off {byte_off} out of byte range"
        );
        let hi = slot_addr + 1;
        // FSR0 = [slot] (16-bit).
        self.emit(format!("    MOVF 0x{slot_addr:02X}, W"));
        self.emit("    MOVWF FSR0L".to_string());
        self.emit(format!("    MOVF 0x{hi:02X}, W"));
        self.emit("    MOVWF FSR0H".to_string());
        // W = k + byte_off + Σ terms (the offset, 8-bit).
        match terms {
            [] => {
                if kk != 0 {
                    self.emit(format!("    MOVLW 0x{kk:02X}"));
                } else {
                    self.emit("    MOVLW 0x00".to_string());
                }
            }
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
                if kk != 0 {
                    self.emit(format!("    ADDLW 0x{kk:02X}"));
                }
            }
        }
        // FSR0 += W with carry into FSR0H.
        self.emit("    ADDWF FSR0L, F".to_string());
        self.emit("    BTFSC STATUS, 0".to_string());
        self.emit("    INCF FSR0H, F".to_string());
    }

    /// Accumulates scaled terms into scratch. Reloads W per repetition:
    /// ADDWF consumes W, so reuse without reload folds the wrong sum and
    /// mis-addresses.
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

    /// Computes the const-table byte index into W. One scale-1 term keeps
    /// the fast register shape. General sums accumulate in scratch.
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

    /// Reads a large const table via a 16-bit index. Splits the index into
    /// an in-chunk byte and a chunk number. Two chunks test one bit. More
    /// chunks walk a descending threshold chain (epic-cc#8). Parks the index
    /// across the PCLATH set and the byte across the restore. Leaves the
    /// byte in W like the small path. Other index shapes panic: the index
    /// set is closed.
    fn emit_const_read_large(&mut self, name: &str, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: const index k {k} + off {byte_off} out of byte range"
        );
        let size = self.global_size(name) as usize;
        let chunks = (size + 255) / 256; // 256-byte tables: 1 (empty chunk 1)
        let disp = chunks.max(2); // dispatch shape: bit-0 test or >= c chain
                                  // Maps a chunk to its reader entry name.
        let entry = |c: usize| {
            if c == 0 {
                format!("__read_{name}")
            } else if c == 1 {
                format!("__read_{name}_hi")
            } else {
                format!("__read_{name}_hi{c}")
            }
        };
        // Calls one chunk entry and parks the byte across the restore.
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
        // Dispatches 3 or more chunks with descending threshold tests.
        // Each test branches to its chunk call. Fall-through reaches chunk
        // 0. Every call lands on the done label.
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
                    // Tests bit 0 of the chunk temp for the two-chunk shape.
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
                // Scales a multi-byte element index without a multiplier.
                // Shifts the low byte and folds the carry for two chunks.
                // Adds the scaled high byte for 3 or more, so the temp holds
                // the chunk number.
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
                    // Adds the scaled high index byte into the chunk temp.
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
                    // Tests bit 0 of the chunk temp for the two-chunk shape.
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
                    // Reads base bytes as a runtime address. An alloca has no
                    // link-time literal, so it panics there. A propagated
                    // global materializes like an address slot (epic-cc#193).
                    if let Base::Global(name) = &base {
                        let addr = self.ptr_value_addr(name, k);
                        let lo = (addr & 0xFF) as u8;
                        let hi = ((addr >> 8) & 0xFF) as u8;
                        match terms.as_slice() {
                            [] => {
                                self.emit(format!(
                                    "    MOVLW 0x{:02X}",
                                    if idx == 0 { lo } else { hi }
                                ));
                            }
                            [(1, reg)] => {
                                let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                if idx == 0 {
                                    self.emit(format!("    MOVLW 0x{lo:02X}"));
                                    self.emit(format!("    ADDWF 0x{ra:02X}, W"));
                                } else {
                                    self.emit(format!("    MOVLW 0x{hi:02X}"));
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
                                    self.emit(format!("    MOVLW 0x{lo:02X}"));
                                    self.emit(format!("    ADDWF 0x{ra1:02X}, W"));
                                    self.emit(format!("    ADDWF 0x{ra2:02X}, W"));
                                } else {
                                    self.emit(format!("    MOVLW 0x{hi:02X}"));
                                    self.emit("    BTFSC STATUS, 0".to_string());
                                    self.emit("    ADDLW 0x01".to_string());
                                    self.emit(format!("    ADDWF 0x{:02X}, W", ra1 + 1));
                                    self.emit(format!("    ADDWF 0x{:02X}, W", ra2 + 1));
                                }
                            }
                        }
                        return;
                    }
                    let sa = match &base {
                        Base::Slot(sname, indirect) => {
                            assert!(
                                *indirect || self.param_holds_addr(sname),
                                "isel: cannot take the value of a GEP over {base:?}"
                            );
                            self.slot_addr(self.cur_func, sname).direct()
                        }
                        other => panic!("isel: cannot take the value of a GEP over {other:?}"),
                    };
                    // Builds `base + k + terms` with carry into byte 1. Byte 0
                    // produces carry only when it adds. A bare move leaves
                    // stale carry, so propagation stays conditional.
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
                self.emit_w_load(a + u16::from(idx));
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    // Materializes a function address as link-time literals
                    // (epic-cc#73).
                    let lit = if idx == 0 { "LOW" } else { "HIGH" };
                    self.emit(format!("    MOVLW {lit}({g})"));
                } else {
                    // Materializes a data global as its address literals,
                    // never the pointee contents. Straddling globals use the
                    // linear alias (docs/33 §D-2) (epic-cc#155).
                    let a = self.ptr_value_addr(g, 0);
                    let b = ((a >> (idx as u32 * 8)) & 0xFF) as u8;
                    self.emit(format!("    MOVLW 0x{b:02X}"));
                }
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
            self.emit_w_store(dst + u16::from(i));
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

    /// Sets C to (a >= b), signed or unsigned. Wide widths route through
    /// the borrow-accurate wide emitters (epic-cc#1). Only the i8 path stays
    /// here. A const RHS becomes the subtrahend. A const LHS uses SUBLW
    /// since a const never names a file register.
    fn emit_cmp_c(&mut self, a: &Val, b: &Val, ty: Ty, signed: bool) {
        let n = ty.bytes();
        let high = n - 1;
        match (a, b) {
            (Val::Const(_), Val::Const(_)) => panic!("isel: constant folding not implemented"),
            (Val::Const(k), _) => {
                if n > 1 {
                    // Routes wide const-LHS chains to the wrap-correct fold.
                    // One byte has no borrow chain, so it stays inline.
                    self.emit_cmp_c_const_lhs_wide(k, b, n, high, signed);
                    return;
                }
                // Subtracts the b byte from the const byte, so C holds the
                // unsigned result.
                self.emit_load_cmp_byte(b, 0, signed, high);
                let k0 = (k & 0xFF) as u8;
                // Folds the sign complement into the i8 low byte when signed.
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

    /// Folds the wide file-LHS borrow with the wrap-correct skip. The naive
    /// fold corrupts borrow-out at the wrap, so INCFSZ preserves the true
    /// borrow. A compare leaves only flags, so folding on the operand is
    /// safe. The high byte complements both sides for signed order.
    fn emit_cmp_c_file_lhs_wide(&mut self, a: &Val, b: &Val, n: u8, high: u8, signed: bool) {
        let aa = self.val_addr(a).direct();
        // Byte 0 has no borrow-in; a single SUBWF leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    SUBWF 0x{aa:02X}, W"));
        for i in 1..n {
            if signed && i == high {
                // Complements both sides at the high byte and folds the
                // complemented b-side through the skip, keeping the true
                // borrow-out.
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
                // Reloads the complemented b-side after the a-side clobbers W.
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

    /// Folds the wide const-LHS borrow with the same wrap-correct skip.
    /// Complements the signed high literal and the b-side into the temp,
    /// so the skip keeps the true borrow-out.
    fn emit_cmp_c_const_lhs_wide(&mut self, k: &i64, b: &Val, n: u8, high: u8, signed: bool) {
        // Byte 0 has no borrow-in; a single SUBLW leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        let k0 = (k & 0xFF) as u8;
        let k0 = if signed && high == 0 { k0 ^ 0x80 } else { k0 };
        self.emit(format!("    SUBLW 0x{k0:02X}"));
        for i in 1..n {
            if signed && i == high {
                // Complements the b-side into the temp and folds it through
                // the skip for the true borrow-out.
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

    /// Materializes a flag predicate into `dst`. Reads C and Z from the
    /// preceding compare. Only flag-neutral moves may sit between. Covers
    /// equality, unsigned and signed orders, and their negations.
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
            // Adjusts the two-condition shapes with the Z test: equality
            // clears or sets the provisional 1.
            self.emit("    BTFSC STATUS, 2 ; Z".to_string());
            self.emit(format!("    {adj2}"));
        }
        self.emit(format!("    MOVWF 0x{dst:02X}"));
    }

    /// Branches on `cond`: zero goes to `f`, nonzero to `t`. Folds
    /// constant conditions to a direct jump.
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

    /// Selects `a` or `b` into `d` through a conditional jump. Folds
    /// constant conditions. Routes arms through the value mover, which
    /// handles const and reg without flag use.
    ///
    /// Reports whether `name` names a seeded indirect slot holding a
    /// runtime address value rather than a folded pointer.
    fn select_is_seeded(&self, name: &str) -> bool {
        matches!(
            self.resolved.get(&ssa_key(self.cur_func, name)),
            Some((Base::Slot(_, true), 0, t)) if t.is_empty()
        )
    }

    /// Copies the two-byte address value of `val` into `dst`. Handles
    /// literals, link-time addresses, and runtime address slots
    /// (epic-cc#147). A computed address with terms panics: it names no
    /// single value.
    fn emit_move_addr_to_slot(&mut self, val: &Val, dst: u16) {
        match val {
            Val::Const(k) => {
                self.emit(format!("    MOVLW 0x{:02X}", (k & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", dst));
                self.emit(format!("    MOVLW 0x{:02X}", ((k >> 8) & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
            Val::Global(g) => {
                if self.is_function(g) {
                    self.emit(format!("    MOVLW LOW({g})"));
                    self.emit(format!("    MOVWF 0x{:02X}", dst));
                    self.emit(format!("    MOVLW HIGH({g})"));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
                } else {
                    let addr = self.ptr_value_addr(g, 0);
                    self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                    self.emit(format!("    MOVWF 0x{:02X}", dst));
                    self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                    self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
                }
            }
            Val::Reg(r) => {
                let (base, k, terms) = self.resolved_for(r);
                assert!(
                    k == 0 && terms.is_empty(),
                    "isel: cannot materialize a computed address ({base:?} k={k} terms={terms:?}) as a select arm"
                );
                let sa = match &base {
                    Base::Slot(sname, true) => self.slot_addr(self.cur_func, sname).direct(),
                    Base::Global(name) => {
                        let addr = self.ptr_value_addr(name, k);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", dst));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
                        return;
                    }
                    other => panic!("isel: cannot materialize {other:?} as a select arm"),
                };
                self.emit(format!("    MOVF 0x{sa:02X}, W"));
                self.emit(format!("    MOVWF 0x{:02X}", dst));
                self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                self.emit(format!("    MOVWF 0x{:02X}", dst + 1));
            }
        }
    }

    /// Selects `a` or `b` into `d` through a conditional jump. Folds
    /// constant conditions. Materializes address bytes when `dst` is a
    /// seeded indirect slot, else copies value bytes (epic-cc#147).
    fn emit_select(&mut self, dst: &str, cond: &Val, ty: Ty, a: &Val, b: &Val) {
        let da = self.slot_addr(self.cur_func, dst).direct();
        let addr_value = self.select_is_seeded(dst);
        match cond {
            Val::Const(k) => {
                let v = if *k != 0 { a } else { b };
                if addr_value {
                    self.emit_move_addr_to_slot(v, da);
                } else {
                    self.emit_move_val_to_slot(v, ty, da);
                }
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
        if addr_value {
            self.emit_move_addr_to_slot(a, da);
        } else {
            self.emit_move_val_to_slot(a, ty, da);
        }
        self.emit(format!("    GOTO {l_end}"));
        self.emit(format!("{l_else}:"));
        if addr_value {
            self.emit_move_addr_to_slot(b, da);
        } else {
            self.emit_move_val_to_slot(b, ty, da);
        }
        self.emit(format!("{l_end}:"));
    }

    /// Returns a fresh intra-block label. Scopes the counter to the module
    /// so labels stay unique across functions.
    fn fresh_label(&mut self) -> String {
        let s = format!("tmp{}", *self.tmp);
        *self.tmp += 1;
        s
    }

    /// Adds two i16 values with carry from low to high byte. Accepts one
    /// register plus one register or const.
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

    /// Applies a commutative bytewise op at i8 or i16. Swaps a const LHS to
    /// the literal path, so no const names a file register. Takes the
    /// file mnemonic and the literal mnemonic.
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
                    self.emit_w_load(bb + u16::from(i));
                    self.emit(format!("    {op} 0x{:02X}, W", ra + u16::from(i)));
                    self.emit_w_store(dst + u16::from(i));
                }
            }
            Val::Const(k) => {
                for i in 0..n {
                    let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                    self.emit_w_load(ra + u16::from(i));
                    self.emit(format!("    {opw} 0x{byte:02X}"));
                    self.emit_w_store(dst + u16::from(i));
                }
            }
            Val::Global(_) => panic!("isel: {op} with a global operand"),
        }
    }

    /// Subtracts i8 values with SUBWF. Keeps the minuend as the file
    /// operand. The caller rejects const LHS shapes.
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a).direct();
        match b {
            Val::Const(k) => {
                // Masks the byte: negative i8 prints hold the same mod-256
                // value.
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

    /// Subtracts a const LHS across bytes with SUBLW and the wrap-correct
    /// skip. Preloads each minuend byte into the destination and folds the
    /// borrow through scratch, keeping the true borrow-out at the wrap
    /// (epic-cc#1).
    fn emit_sub_const_lhs(&mut self, k: &i64, a: &Val, dst: u16, bytes: u8) {
        let aa = self.val_addr(a).direct();
        self.emit(format!("    MOVF 0x{aa:02X}, W"));
        self.emit(format!("    SUBLW 0x{:02X}", (k & 0xFF) as u8));
        self.emit(format!("    MOVWF 0x{dst:02X}"));
        for i in 1..bytes {
            let kb = ((k >> (i as u32 * 8)) & 0xFF) as u8;
            // Stages the subtrahend in scratch and reloads W before the fold,
            // so the no-borrow path subtracts the right bytes.
            self.emit(format!("    MOVF 0x{:02X}, W", aa + u16::from(i)));
            self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            self.emit(format!("    MOVLW 0x{kb:02X}"));
            self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(i)));
            self.emit(format!("    MOVF 0x{:02X}, W", self.scratch));
            self.emit("    BTFSS STATUS, 0 ; C".to_string());
            self.emit(format!("    INCFSZ 0x{:02X}, W", self.scratch));
            self.emit(format!("    SUBWF 0x{:02X}, F", dst + u16::from(i)));
        }
    }

    /// Subtracts i16 values with the low borrow folded into the high byte.
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

    /// Adds i32 values with the carry folded per byte through scratch.
    /// Uses the skip fold so the wrap keeps the true carry-out.
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

    /// Subtracts i32 values with the borrow folded per byte through scratch.
    /// Uses the skip fold so the wrap keeps the true borrow-out.
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
                    // Stages the subtrahend in scratch before the preload
                    // clobbers W.
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

    /// Copies each call arg into the callee param slots. Serves direct
    /// calls and per-candidate indirect arms (epic-cc#73).
    fn emit_call_args(&mut self, func: &str, args: &[ir::CallArg]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        let named = callee.params.len();
        let mut va_off: u16 = 0;
        for (i, arg) in args.iter().enumerate() {
            if i >= named {
                // Places an extra variadic arg in the callee `__va` region
                // at the running offset (epic-cc#131). The region stays in
                // one address pair by construction.
                let va = self
                    .addrs
                    .get(&ssa_key(func, "__va"))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!("isel: variadic call to non-variadic @{func} (no __va region)")
                    });
                let aty = arg.ty.expect("isel: scalar variadic arg must carry a type");
                let aw = u16::from(aty.bytes());
                assert!(
                    (va & 0xFF) + va_off + aw <= 0x100,
                    "isel: variadic args of @{func} exceed its va region (off {va_off} + {aw})"
                );
                self.emit_move_val_to_slot(&arg.val, aty, va + va_off);
                va_off += aw;
                continue;
            }
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(func, pname).direct();
            if let Some(size) = arg.byval {
                // Copies a byval arg byte by byte: the param slot is the
                // callee struct copy.
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
                // Stores an sret target address into the callee slot. The
                // target fits one GPR window, so FSR plus IRP reaches it
                // without crossing a hole.
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
                // Loads the callee address from the stored bytes, so the
                // target fits one bank by construction.
                assert!(
                    !object_straddles(self.device, addr, span),
                    "isel: sret target at 0x{addr:03X} span {span} straddles a bank; \
                     sret targets must fit one GPR bank"
                );
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
                        if self.is_function(g) {
                            // Materializes a function address as link-time
                            // literals, including forwarded callbacks
                            // (epic-cc#73) (epic-cc#137).
                            self.emit(format!("    MOVLW LOW({g})"));
                            self.emit(format!("    MOVWF 0x{:02X}", pa));
                            self.emit(format!("    MOVLW HIGH({g})"));
                            self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                        } else {
                            if self.global_is_const(g) {
                                let size = self.global_size(g);
                                panic!("isel: const global @{g} too large for RAM copy ({size} bytes, max 255)");
                            }
                            let addr = self.ptr_value_addr(g, 0);
                            self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                            self.emit(format!("    MOVWF 0x{:02X}", pa));
                            self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                            self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                        }
                    }
                    Val::Const(c) => {
                        assert_eq!(*c, 0, "isel: non-zero const ptr not supported");
                        self.emit(format!("    CLRF 0x{:02X}", pa));
                        self.emit(format!("    CLRF 0x{:02X}", pa + 1));
                    }
                    // Materializes a global at a constant offset as literals.
                    // Covers GEPs over const globals copied to RAM.
                    Val::Reg(r) if !self.resolved.contains_key(&ssa_key(self.cur_func, r)) => {
                        // Copies a runtime pointer value from its slot. The
                        // callee derefs it through FSR at runtime
                        // (epic-cc#155).
                        let sa = self.slot_addr(self.cur_func, r).direct();
                        self.emit(format!("    MOVF 0x{sa:02X}, W"));
                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                        self.emit(format!("    MOVF 0x{:02X}, W", sa + 1));
                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                    }
                    Val::Reg(r) if matches!(self.resolved_for(r), (Base::Global(_), _, ref t) if t.is_empty()) =>
                    {
                        let (base, k, _) = self.resolved_for(r);
                        let Base::Global(name) = &base else {
                            unreachable!()
                        };
                        let addr = self.ptr_value_addr(name, k);
                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                    }
                    Val::Reg(r) => {
                        let (base, k, terms) = self.resolved_for(r);
                        // Reads the base slot as a runtime address. A GEP over
                        // a RAM-copied const stays a RAM address with offset.
                        let sa = match &base {
                            Base::Global(name) => {
                                // Treats a RAM-copied const like a slot base.
                                // Covers const offsets and single terms.
                                let k_lo = (u16::from(k) & 0xFF) as u8;
                                let k_hi = (u16::from(k) >> 8) as u8;
                                match terms.as_slice() {
                                    [] => {
                                        let addr = self.ptr_value_addr(name, k);
                                        self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                                        self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                                        continue;
                                    }
                                    [(1, reg)] => {
                                        // Uses the linear alias for straddling
                                        // bases, so the offset walks the
                                        // linear region (docs/33 §D-2).
                                        let base_addr = self.ptr_value_addr(name, 0);
                                        let ra = self.val_addr(&Val::Reg(reg.clone())).direct();
                                        self.emit(format!("    MOVLW 0x{:02X}", (base_addr & 0xFF) as u8));
                                        self.emit(format!("    ADDWF 0x{:02X}, W", ra));
                                        if k_lo != 0 {
                                            self.emit(format!("    ADDLW 0x{k_lo:02X}"));
                                        }
                                        self.emit(format!("    MOVWF 0x{:02X}", pa));
                                        self.emit(format!("    MOVLW 0x{:02X}", ((base_addr >> 8) & 0xFF) as u8));
                                        self.emit("    BTFSC STATUS, 0".to_string());
                                        self.emit("    ADDLW 0x01".to_string());
                                        self.emit(format!("    ADDWF 0x{:02X}, W", ra + 1));
                                        if k_hi != 0 {
                                            self.emit(format!("    ADDLW 0x{k_hi:02X}"));
                                        }
                                        self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                                        continue;
                                    }
                                    _ => panic!("isel: plain ptr arg with multiple terms not yet supported: {terms:?}"),
                                }
                            }
                            Base::Slot(sname, false) if self.param_holds_addr(sname) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
                            Base::Slot(sname, false) => {
                                // Materializes an alloca address as literals:
                                // the slot is the object (epic-cc#125).
                                let addr = self.slot_addr(self.cur_func, sname).direct();
                                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                                self.emit(format!("    MOVWF 0x{:02X}", pa));
                                self.emit(format!(
                                    "    MOVLW 0x{:02X}",
                                    ((addr >> 8) & 0xFF) as u8
                                ));
                                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
                                continue;
                            }
                            // Passes runtime address bytes through
                            // (epic-cc#147).
                            Base::Slot(sname, true) => {
                                self.slot_addr(self.cur_func, sname).direct()
                            }
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
                // Extends a narrow conversion source to a full 4-byte slot:
                // unsigned sources zero-extend, signed sources sign-extend,
                // so the recipe reads a defined i32.
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
    }

    /// Calls `func` with args in callee slots, then copies retval bytes
    /// into `dst`. Skips the copy for void calls.
    fn emit_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        if !callees.is_empty() {
            self.emit_indirect_call(dst, ty, func, args, callees);
            return;
        }
        // Emits a trap for an unresolvable indirect target. A valid program
        // never reaches it. The trap stays deterministic rather than calling
        // nothing (epic-cc#137).
        if !self.is_function(func) {
            let l_trap = self.fresh_label();
            self.emit(format!("{l_trap}:"));
            self.emit(format!("    GOTO {l_trap}"));
            return;
        }
        self.emit_call_args(func, args);
        // Runs each CALL with PCLATH set to the target page after arg copies
        // (the set clobbers W), then restores the caller page unless the
        // target shares it.
        self.emit(format!("    MOVLW PAGE({func})"));
        self.emit("    MOVWF PCLATH".to_string());
        self.emit(format!("    CALL {func}"));
        self.emit_pclath_restore(func);
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
            // Copies the retval bytes into the destination slots.
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
    /// Calls through a function pointer with a compare-and-call chain.
    /// Compares both address bytes per candidate, then runs the direct
    /// sequence on a match. Falls through to a trap when no candidate
    /// matches: a valid program never reaches it (epic-cc#73).
    fn emit_indirect_call(
        &mut self,
        dst: &Option<String>,
        ty: Option<Ty>,
        func: &str,
        args: &[ir::CallArg],
        callees: &[String],
    ) {
        let fp = self.slot_addr(self.cur_func, func).direct();
        let l_done = self.fresh_label();
        for cand in callees.iter() {
            let l_next = self.fresh_label();
            // Compares both address bytes with no memory op between flag
            // and skip, so banking inserts nothing inside the pair
            // (epic-cc#6).
            self.emit(format!("    MOVF 0x{fp:02X}, W"));
            self.emit(format!("    XORLW LOW({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            self.emit(format!("    MOVF 0x{:02X}, W", fp + 1));
            self.emit(format!("    XORLW HIGH({cand})"));
            self.emit("    BTFSS STATUS, 2 ; Z".to_string());
            self.emit(format!("    GOTO {l_next}"));
            // Runs the direct sequence for the matched candidate.
            self.emit_call_args(cand, args);
            self.emit(format!("    MOVLW PAGE({cand})"));
            self.emit("    MOVWF PCLATH".to_string());
            self.emit(format!("    CALL {cand}"));
            self.emit_pclath_restore(cand);
            self.emit(format!("    GOTO {l_done}"));
            self.emit(format!("{l_next}:"));
        }
        // Traps when no candidate matches.
        let l_trap = self.fresh_label();
        self.emit(format!("{l_trap}:"));
        self.emit(format!("    GOTO {l_trap}"));
        self.emit(format!("{l_done}:"));
        if let Some(d) = dst {
            let t = ty.expect("isel: valued call must carry a type");
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
        self.cur_loc = i.loc().cloned();
        match i {
            Inst::Load(l) => {
                // Treats i1 as one byte like i8 throughout (epic-cc#304).
                let ty = if l.ty == Ty::I1 { Ty::I8 } else { l.ty };
                let dst = self.slot_addr(self.cur_func, &l.dst).direct();
                if let Some(g) = l.ptr.strip_prefix('@') {
                    let src = self.global_addr(g);
                    for k in 0..ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(k)));
                        self.emit_w_store(dst + u16::from(k));
                    }
                } else if l.ptr.starts_with("0x") {
                    // Reads an SFR literal directly with no FSR setup.
                    // Banking supplies the address mode. Only the result
                    // slot stays tracked.
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", base + u16::from(k)));
                        self.emit_w_store(dst + u16::from(k));
                    }
                } else {
                    // Routes GEP pointers by base: const bases take the
                    // table path, RAM bases use direct or FSR access.
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global, %reg or a literal", l.ptr)
                    });
                    let ptr = Val::Reg(r.to_string());
                    for k in 0..ty.bytes() {
                        self.emit_ptr_load_byte(&ptr, k);
                        self.emit_w_store(dst + u16::from(k));
                    }
                }
            }
            Inst::Store(s) => {
                let ty = if s.ty == Ty::I1 { Ty::I8 } else { s.ty };
                if let Some(g) = s.ptr.strip_prefix('@') {
                    let dst = self.global_addr(g);
                    self.emit_move_val_to_slot(&s.val, ty, dst);
                } else if s.ptr.starts_with("0x") {
                    // Writes an SFR literal directly with no FSR setup.
                    let base = literal_ptr_addr(&s.ptr);
                    for k in 0..ty.bytes() {
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
                    for k in 0..ty.bytes() {
                        self.emit_ptr_store_byte(&ptr, k, &s.val);
                    }
                }
            }
            Inst::Gep(_) => {} // virtual: lowered at each load/store use
            Inst::Alloca(_) => {} // virtual: the slot is sized by alloc; lowered at each use
            Inst::Memcpy(m) => match &m.len {
                MemLen::Const(n) => {
                    // Loops bytes through the pointer machinery, re-resolving
                    // each side per byte. Parks the byte when FSR setup
                    // clobbers W (docs/33 §D-2). Direct sides need no park.
                    let hold: u16 = 0x7F;
                    for i in 0..*n {
                        self.emit_ptr_load_byte(&m.src, i);
                        if self.ptr_setup_is_indirect(&m.dst, i) {
                            // Parks the byte across FSR setup, which clobbers W.
                            self.emit(format!("    MOVWF 0x{hold:02X}"));
                            self.emit_ptr_setup(&m.dst, i);
                            self.emit(format!("    MOVF 0x{hold:02X}, W"));
                            self.emit("    MOVWF INDF0".to_string());
                        } else {
                            // Keeps W live: direct setup emits nothing.
                            let a = match self.emit_ptr_setup(&m.dst, i) {
                                Addr::Direct(a) => a,
                                Addr::Indirect => unreachable!(),
                            };
                            self.emit(format!("    MOVWF 0x{a:02X}"));
                        }
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
                        // Swaps a const LHS to the RHS, so no const names
                        // a file register.
                        let (a, b_op) = match (&b.a, &b.b) {
                            (Val::Const(_), Val::Const(_)) => {
                                panic!("isel: constant folding not implemented")
                            }
                            (Val::Const(_), _) => (&b.b, &b.a),
                            _ => (&b.a, &b.b),
                        };
                        match b_op {
                            Val::Const(k) => {
                                // Masks to the byte: signed prints hold the
                                // same mod-256 value.
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
                    // Shares one emitter for commutative bytewise ops. The
                    // emitter swaps const sides itself.
                    (BinOp::And, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::And, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::And, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "ANDWF", "ANDLW"),
                    (BinOp::Or, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Or, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Or, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "IORWF", "IORLW"),
                    (BinOp::Xor, Ty::I8) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    (BinOp::Xor, Ty::I16) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    (BinOp::Xor, Ty::I32) => self.emit_commutative(&b.a, &b.b, b.ty, da, "XORWF", "XORLW"),
                    // Keeps sub non-commutative: const LHS takes its own
                    // SUBLW path with swapped roles.
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
                    // Rejects mul and div here: legalize routes them to
                    // runtime calls. Reaching isel panics: the legalize
                    // contract is closed.
                    (BinOp::Mul, _) => panic!("isel: mul reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::UDiv, _) => panic!("isel: udiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::URem, _) => panic!("isel: urem reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SDiv, _) => panic!("isel: sdiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SRem, _) => panic!("isel: srem reached isel; legalize must rewrite it to a routine call"),
                    // Inlines const-count shifts as fixed rotates. Rejects
                    // poison counts and variable counts: legalize routes the
                    // latter to runtime calls.
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
            // Copies freeze byte for byte: a backend no-op.
            Inst::Freeze(f) => {
                let da = self.slot_addr(self.cur_func, &f.dst).direct();
                self.emit_move_val_to_slot(&f.val, f.ty, da);
            }
            Inst::Zext(z) => {
                // Treats i1 to i8 as a one-byte copy: compare results already
                // hold 0 or 1. Equal widths copy identically. Narrowing
                // panics: the shape is closed.
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
            Inst::IntToPtr(p) => {
                // Copies a runtime integer address into a seeded indirect
                // slot. Widths match like zext, but the result is an address.
                assert_eq!(
                    p.from, p.to,
                    "isel: inttoptr must keep the byte width (i16 -> ptr)"
                );
                let da = self.slot_addr(self.cur_func, &p.dst).direct();
                for i in 0..p.from.bytes() {
                    self.emit_load_byte(&p.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Sext(x) => {
                // Sign-extends from the source high byte. Rejects i1 sources:
                // a 0 or 1 value names no sign bit, so filling from bit 7
                // panics.
                assert!(
                    x.from != Ty::I1 && x.from.bytes() < x.to.bytes(),
                    "isel: sext only supports i8/i16 -> i16/i32 (i1 sign-fill is undefined)"
                );
                assert!(
                    !matches!(&x.val, Val::Const(_)),
                    "isel: sext of a constant not supported (constant folding not implemented)"
                );
                let da = self.slot_addr(self.cur_func, &x.dst).direct();
                // Copies low bytes unchanged.
                for i in 0..x.from.bytes() {
                    self.emit_load_byte(&x.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                // Fills high bytes from the source sign bit once, then
                // stores the fill into each high byte.
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
                // Distinguishes narrowing trunc from widening by type, since
                // i1 and i8 share one byte.
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
                    // Masks to one bit: consumers test nonzero on the byte.
                    self.emit("    MOVLW 0x01".to_string());
                    self.emit(format!("    ANDWF 0x{da:02X}, F"));
                }
            }
            Inst::Icmp(ic) => {
                let da = self.slot_addr(self.cur_func, &ic.dst).direct();
                match ic.pred.as_str() {
                    "eq" => {
                        // Compares equality through XOR and materializes Z.
                        self.emit_cmp_eq(&ic.a, &ic.b, ic.ty);
                        self.emit_materialize("Z", da);
                    }
                    "ne" => {
                        // Inverts the equality materialization for ne.
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
                        // C = (a >= b), unsigned or signed (sign-bit complement).
                        self.emit_cmp_c(&ic.a, &ic.b, ic.ty, signed);
                        // A multi-byte borrow chain ends with a byte-level Z;
                        // full equality needs the XOR accumulation, which
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
                if s.ptr {
                    if matches!((&s.a, &s.b), (Val::Const(_), Val::Const(_))) {
                        // Materializes address bytes for literal pointer arms
                        // into the seeded slot.
                        self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                    } else if self.select_is_seeded(&s.dst) {
                        // Materializes runtime address arms into the seeded
                        // slot when folding cannot apply (epic-cc#147).
                        self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                    } else {
                        // Folds pointer selects into the resolved map and
                        // emits nothing: uses lower through the fold.
                    }
                } else {
                    self.emit_select(&s.dst, &s.cond, s.ty, &s.a, &s.b);
                }
            }
            Inst::Call(c) => self.emit_call(&c.dst, c.ty, &c.func, &c.args, &c.callees),
            Inst::VaStart(v) => {
                // Stores the region base into the list slot. Forwarded lists
                // arrive as pointer params and skip local setup.
                let list = self.slot_addr(self.cur_func, &v.list).direct();
                let va_base = self
                    .addrs
                    .get(&ssa_key(self.cur_func, "__va"))
                    .copied()
                    .unwrap_or_else(|| {
                        panic!(
                            "isel: va_start in non-variadic context {} (no __va region)",
                            self.cur_func
                        )
                    });
                // Keeps the region in one address pair for FSR access.
                let region_w = self.func_va_size(self.cur_func);
                assert!(
                    (va_base & 0xFF) + region_w <= 0x100,
                    "isel: va region @0x{va_base:02X} ({} bytes) crosses a 256-byte GPR pair",
                    region_w
                );
                self.emit(format!("    MOVLW 0x{:02X}", (va_base & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{list:02X}"));
                self.emit(format!("    MOVLW 0x{:02X}", ((va_base >> 8) & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", list + 1));
            }
            Inst::VaArg(v) => {
                // Reads the next arg through the list address, then advances
                // the address by the width. Covers local and forwarded lists.
                let da = self.slot_addr(self.cur_func, &v.dst).direct();
                let list = self.slot_addr(self.cur_func, &v.ptr).direct();
                for i in 0..v.ty.bytes() {
                    self.emit_fsr_indirect(list, 0, &[], i);
                    self.emit("    MOVF INDF0, W".to_string());
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                // Advances with carry from low to high byte via the Z flag.
                for _ in 0..v.ty.bytes() {
                    self.emit(format!("    INCF 0x{list:02X}, F"));
                    self.emit("    BTFSC STATUS, 2".to_string());
                    self.emit(format!("    INCF 0x{:02X}, F", list + 1));
                }
            }
            Inst::Asm(a) => {
                // Emits inline asm verbatim with substituted memory operands.
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
        self.cur_loc = t.loc().cloned();
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
            Inst::Ret(None, _) => self.emit("    RETURN".to_string()),
            Inst::Ret(Some((ty, v)), _) => {
                // Copies the return value into fixed retval slots, then
                // returns.
                for i in 0..ty.bytes() {
                    self.emit_load_byte(v, i);
                    self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo + u16::from(i)));
                }
                self.emit("    RETURN".to_string());
            }
            _ => panic!("isel: unsupported terminator for milestone 2"),
        }
    }

    // Soft integer routine recipes: mul, div, and rem.

    /// Verifies every recipe slot sits in one GPR bank (epic-cc#6).
    /// Skip-sensitive loops break when banking splits a test from its
    /// target, so `alloc` rounds each frame into one bank and this check
    /// enforces it. A straddle panics: silent crossing miscompiles.
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

    /// Runs the shared 32-iteration restoring division loop. Shifts the
    /// quotient into the numerator slots while accumulating the partial
    /// remainder. Holds denominator, remainder, and count in scratch.
    /// Leaves carry as the quotient-bit decision after the final byte.
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

    /// Emits one mul, div, or rem routine body over param slots with results
    /// in retval and state in scratch. Uses plain addresses for banking.
    /// Runs div-by-zero loops without guards: the result is poison, so any
    /// deterministic value satisfies the contract. Shares shift bodies
    /// through the shift emitter.
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
            // Covers the soft-float add and sub recipes with round to
            // nearest even. Args arrive in param slots, results leave in
            // retval, scratch lives in `__scr`. Slots stay in one bank for
            // skip-sensitive loops (epic-cc#6).
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

    /// Emits one variable-count shift routine body. Masks the raw count to
    /// the width minus one, so the loop stays bounded and out-of-range
    /// counts yield defined results. Shifts in place in the `val` slot.
    /// Fills sign bits for arithmetic shifts. Keeps slots in one bank for
    /// skip-sensitive loops (epic-cc#6).
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
            // Clears the high count byte: the masked count fits the low
            // byte, so stale high bytes must not leak into the loop.
            self.emit(format!("    CLRF 0x{:02X}", scr + 1));
        }
        let l_loop = self.fresh_label();
        let l_done = self.fresh_label();
        // Skips the loop on a zero count: a bottom-tested loop runs once
        // otherwise.
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

    // Soft-float recipes for single precision. Layout is little-endian with
    // sign, exponent, and mantissa fields plus an implicit leading bit on
    // nonzero exponents. Rounds to nearest even, renormalizing on carry.
    // Results leave in the fixed retval region.

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

    /// Splits an f32 slot into sign, biased exponent, and 24-bit mantissa.
    /// Clears the mantissa for zero exponents. Flips the sign for
    /// subtraction. Keeps denormal fractions without the implicit bit
    /// (epic-cc#11).
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
        // Loads the high mantissa byte with the implicit bit, except for
        // denormals, which carry no implicit bit.
        self.emit(format!("    MOVF 0x{:02X}, W", slot));
        self.emit(format!("    MOVWF 0x{:02X}", mant));
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 1));
        self.emit(format!("    MOVWF 0x{:02X}", mant + 1));
        self.emit_f32_mant_hi(slot);
        self.emit(format!("    MOVWF 0x{:02X}", mant + 2));
        // Aligns denormals at the exp-1 scale with the raw fraction. Zero
        // stays at exp 0.
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

    /// Loads the high mantissa byte with the implicit bit, except for
    /// denormals, which carry no implicit bit (epic-cc#11). The caller
    /// stores W into the mantissa top.
    fn emit_f32_mant_hi(&mut self, slot: u16) {
        let l_imp = self.fresh_label();
        let l_done = self.fresh_label();
        // Checks for exp 0 across both exponent bytes.
        self.emit(format!("    MOVF 0x{:02X}, W", slot + 3));
        self.emit("    ANDLW 0x7F".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_imp}"));
        self.emit(format!("    BTFSC 0x{:02X}, 7", slot + 2));
        self.emit(format!("    GOTO {l_imp}"));
        // Keeps only the fraction for denormals.
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

    /// Emits the add and sub bodies over extracted operands at contract
    /// offsets. Aligns the smaller mantissa with exact lost-fraction
    /// tracking, then adds or subtracts at the larger scale. Reads round
    /// and sticky bits from the tracking window for exact rounding.
    /// Reuses the dead exponent slot for the tracking top.
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
        // Handles NaN and infinity operands first (epic-cc#11).
        // Tests the fraction without the implicit bit to separate NaN from
        // infinity. Uses the dead temp for the OR accumulation.
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
        // Routes infinity operands after the NaN checks: nonzero fractions
        // already left for NaN.
        self.emit(format!("    MOVF 0x{ea:02X}, W"));
        self.emit("    SUBLW 0xFF".to_string());
        self.emit("    BTFSS STATUS, 2".to_string());
        self.emit(format!("    GOTO {l_a_not_inf}"));
        // Combines infinities by sign: same signs yield infinity, else NaN.
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
        // Returns infinity for a finite pair with one infinite side.
        self.emit(format!("    GOTO {l_inf}"));
        self.emit(format!("{l_a_not_inf}:"));
        // Returns infinity with the infinite side sign.
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
        // RNE: round up iff round && (sticky || mantissa LSB), where
        // sticky = OR(ta below the top bit) | stick bit 1 (round = ta2
        // bit 7, the last shifted-out bit; a sum carry promotes it into
        // stick bit 1). Denormal: exp 1 with the top mantissa bit clear
        // (a denormal sum/difference or an underflowed normal) converts
        // to exp 0, the mantissa being the raw fraction assembled as-is;
        // a rounded-up 0x800000 keeps exp 1. Both paths reach here: the
        // add path directly, the sub path after its normalize loop stops at ea == 1.
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
        // Holds the low addend shifted per iteration, starting at zero and
        // inserting each multiplier bit at the top.
        self.emit(format!("    CLRF 0x{:02X}", pb));
        self.emit(format!("    CLRF 0x{:02X}", pb + 1));
        self.emit(format!("    CLRF 0x{:02X}", pb + 2));
        for addr in [m0, m1, m2, m3, low0, low1, low2] {
            self.emit(format!("    CLRF 0x{addr:02X}"));
        }
        self.emit("    MOVLW 0x18".to_string());
        self.emit(format!("    MOVWF 0x{cnt:02X}"));
        self.emit(format!("{l_loop}:"));
        // Tests one multiplier bit per iteration through the shift chain.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RLF 0x{bk0:02X}, F"));
        self.emit(format!("    RLF 0x{bk1:02X}, F"));
        self.emit(format!("    RLF 0x{bk2:02X}, F"));
        self.emit("    BTFSS STATUS, 0".to_string());
        self.emit(format!("    GOTO {l_skip}"));
        // Adds the low part without carry on the first byte: C holds the
        // tested bit there. Later bytes fold the prior carry normally.
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
        // Carries bit 23 of the low sum into the product, not the byte
        // carry-out. Masks the bit out of the low part, which stays modulo
        // its width.
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
        // Shifts the addend down and inserts the next multiplier bit at the
        // top.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pb + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pb + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pb));
        self.emit(format!("    BTFSC 0x{:02X}, 0", pa));
        self.emit(format!("    BSF 0x{:02X}, 6", pb + 2));
        // Shifts the multiplicand down for the next iteration.
        self.emit("    BCF STATUS, 0".to_string());
        self.emit(format!("    RRF 0x{:02X}, F", pa + 2));
        self.emit(format!("    RRF 0x{:02X}, F", pa + 1));
        self.emit(format!("    RRF 0x{:02X}, F", pa));
        self.emit(format!("    DECFSZ 0x{cnt:02X}, F"));
        self.emit(format!("    GOTO {l_loop}"));
        // Unifies the product scale: renormalized products shift right,
        // others drop the leading zero with a left shift.
        self.emit(format!("    BTFSC 0x{m3:02X}, 0"));
        self.emit(format!("    GOTO {l_renorm}"));
        self.emit("    BCF STATUS, 0".to_string());
        // Aligns the low portion so its top becomes the guard bit.
        self.emit(format!("    RLF 0x{low0:02X}, F"));
        self.emit(format!("    RLF 0x{low1:02X}, F"));
        self.emit(format!("    RLF 0x{low2:02X}, F"));
        self.emit(format!("    GOTO {l_norm_check}"));
        self.emit(format!("{l_renorm}:"));
        // Shifts the product right with the guard bit preserved.
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

/// Computes dominator sets for a function CFG. Classifies phi-copy edges:
/// an edge is a back edge when the merge dominates the predecessor, so the
/// predecessor sits in the merge loop and phi slots hold current values.
/// Covers self-loops and separate latch edges.
fn block_dominators(f: &ir::Func) -> HashMap<String, HashSet<String>> {
    let entry = &f.blocks[0].label;
    let all: HashSet<String> = f.blocks.iter().map(|b| b.label.clone()).collect();
    // Builds predecessor lists from terminator targets at block ends.
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

/// Emits one function body into the output. Routes runtime routines to
/// recipes and ordinary functions to labels, phi copies, and terminators.
/// Serves both passes: pass A measures with all restores, pass B skips
/// same-page restores.
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    // Keeps generated labels and glue free of stale source locations from
    // the prior function.
    g.cur_loc = None;
    // Emits routine recipes instead of block bodies. The injected entry
    // holds only a scratch alloca. A name without a recipe panics: the set
    // is closed, and an empty label would fall into the next function.
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
    // Emits naked functions verbatim with no prologue and only asm bodies.
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
    // Names the entry block after the function so calls resolve. Suffixes
    // other blocks to keep labels unique for the assembler.
    let mut labels: HashMap<String, String> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let lbl = if i == 0 {
            f.name.clone()
        } else {
            format!("{}_L{}", f.name, b.label)
        };
        labels.insert(b.label.clone(), lbl);
    }
    // Keys phi copies by edge, not predecessor alone: copies run only on
    // their merge edge, or they clobber slots the other target reads.
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
    // Classifies back edges by dominance: merge slots hold current values
    // there, so readers run first.
    let doms = block_dominators(f);
    for (i, b) in f.blocks.iter().enumerate() {
        g.emit(format!("{}:", labels[&b.label]));
        if i == 0 && f.isr {
            // Saves no core registers in the ISR prologue: hardware preserves
            // them across RETFIE. Backs up ABI retval and scratch, then pins
            // PCLATH to page 0 for intra-function branches (docs/33 §D-4).
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
                Inst::Br(_) | Inst::BrCond(_) | Inst::Ret(..) => terminator = Some(i),
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
                        // Restores ABI state on ISR return: hardware reloads
                        // core registers from shadows, so only retval and
                        // scratch need manual restore (docs/33 §D-4).
                        Inst::Ret(None, _) => {
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
                            g.emit("    RETFIE");
                        }
                        Inst::Ret(Some(_), _) => panic!(
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

/// Emits ordered phi copies for one edge without clobbering live sources.
/// Orders by edge position: back edges read current values first,
/// forward edges define before use. A cyclic swap needs a temp and
/// panics: silent emission miscompiles.
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
                // Orders reader first on back edges: a sibling reading this
                // destination holds a live loop value and runs before the
                // overwrite.
                (0..n).any(|j| !emitted[j] && j != i && pending[j].1 == Some(*da))
            } else {
                // Orders writer first on forward edges: the source comes from
                // this edge alone, so readers follow definers.
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

/// Counts one word per instruction line, ignoring labels, directives,
/// symbol definitions, comments, and blanks. Mirrors the assembler pass-1
/// count so page decisions match assigned addresses.
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
/// Counts init words for RAM-resident const globals: two words per byte.
/// The page-0 base includes this count (epic-cc#207).
fn start_init_words(m: &Module, addrs: &HashMap<String, u16>) -> usize {
    m.globals
        .iter()
        .filter(|g| g.is_const && addrs.contains_key(&g.name))
        .map(|g| 2 * g.bytes.len())
        .sum()
}

/// Verifies the final post-banking layout fits 2048-word pages. Each
/// function stays in one page, or its label and tail disagree and branches
/// misroute (epic-cc#17). Packs on post-banking sizes with anchors, so
/// elision cannot move functions across pages (epic-cc#12). A straddle or
/// overflow panics: the layout contract is closed. Covers `__start` and
/// const readers the same way.
pub fn verify_page_fit(m: &Module, asm: &str) {
    let funcs: Vec<&str> = m.funcs.iter().map(|f| f.name.as_str()).collect();
    // Tracks reader entries as page-checked targets like functions.
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
            // Keeps block labels inside the enclosing function extent.
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
            // Ends the reader extent at table data: tables span pages by
            // design, readers do not.
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

/// Maps each const reader entry to the page PCLATH holds after return.
/// Derives the page from the table base, not the entry address: the entry
/// can sit in another page. Pass B pins the section start, so pages hold
/// across passes.
fn reader_pages(consts: &[&ir::Global], table_start: usize) -> Vec<(String, usize)> {
    let mut pages = Vec::new();
    let mut addr = table_start;
    for g in consts {
        let size = g.bytes.len();
        if size >= 256 {
            // Lays out chunked readers with aligned bases and trailing
            // entries in chunk order.
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
            // Lays out single tables with window alignment, so the return
            // page follows the aligned base.
            let aligned = window_align(addr + 6, size);
            pages.push((format!("__read_{}", g.name), aligned / 0x800));
            addr = aligned + size;
        }
    }
    pages
}

/// Aligns a small table base inside one 256-byte window. Holds the base
/// when it fits, else rounds to the next boundary. Callers emit alignment
/// from this, so placement never splits the window (epic-cc#138).
fn window_align(base: usize, size: usize) -> usize {
    if (base & 0xFF) + size <= 0x100 {
        base
    } else {
        (base + 255) & !255
    }
}

/// Measures the final address after text with assembler pass-1 semantics.
/// Stops at program end or the const section start.
fn measure_end_org(text: &str) -> usize {
    let mut org = 0usize;
    for raw in text.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with("list") || line.starts_with("radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if line.starts_with("end") || line.starts_with("__read_") {
            break;
        }
        if line.strip_suffix(':').is_some() {
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
    org
}

/// Selects instructions for the whole module into PIC14 assembly text.
/// Reads every address from the caller map with no slot allocation.
/// Keeps scratch and retval bytes in fixed common RAM with no banking.
/// Sets PCLATH around each CALL and packs functions into 2048-word pages
/// over post-banking sizes. Measures in pass A, then elides same-page
/// restores in pass B with pinned bases.
pub fn select(device: &Device, m: &Module, addrs: &HashMap<String, u16>) -> String {
    select_with_locs(device, m, addrs).0
}

/// Extends `select` with per-line source locations for the driver address
/// table. Marks generated lines with `None`.
pub fn select_with_locs(
    device: &Device,
    m: &Module,
    addrs: &HashMap<String, u16>,
) -> (String, Vec<Option<SrcLoc>>) {
    let mut out: Vec<String> = Vec::new();
    let mut locs: Vec<Option<SrcLoc>> = Vec::new();
    // Emits the ISR first at the hardware vector: the vector is the entry,
    // since a jump would depend on unknown PCLATH. Extra handlers beyond
    // the vector count panic: one vector serves one handler.
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
    // Pins scratch, retval, and ISR backup to disjoint common-RAM regions
    // with free tail bytes. Hardware shadows cover core registers here
    // (docs/33 §D-4).
    let (common_lo, common_hi) = device
        .common_ram
        .expect("isel's fixed scratch/retval/ISR-save layout needs a common-RAM region");
    let scratch: u16 = common_lo;
    let retval_lo: u16 = common_lo + 1;
    let isr_save_lo: u16 = common_lo + 9;
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
    out.extend(vec![
        "; pic8 -- integer spine milestone 2 (isel)".to_string(),
        format!("    list p={}", device.name),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR0L  equ 0x04".to_string(),
        "FSR0H  equ 0x05".to_string(),
        "FSR1L  equ 0x06".to_string(),
        "FSR1H  equ 0x07".to_string(),
        "INDF0  equ 0x00".to_string(),
        "INDF1  equ 0x01".to_string(),
        "PCL    equ 0x02".to_string(),
        "PCLATH equ 0x0A".to_string(),
        "INTCON equ 0x0B".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ]);
    locs.extend(std::iter::repeat(None).take(12));
    if !m.module_asm.is_empty() {
        out.push("; module asm".to_string());
        locs.push(None);
        for entry in &m.module_asm {
            for line in entry.split('\n') {
                out.push(line.to_string());
                locs.push(None);
            }
        }
    }
    if !has_isr {
        // Places `__start` for the no-ISR layout at the reset reach. Moves
        // it after the ISR when the vector owns its word. Inits RAM-copied
        // const bytes before main runs.
        let mut init: Vec<String> = Vec::new();
        for g in &m.globals {
            if g.is_const && addrs.contains_key(&g.name) {
                let base = addrs[&g.name];
                for (i, b) in g.bytes.iter().enumerate() {
                    // Materializes function address fields as link-time
                    // literals (epic-cc#154).
                    if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                        let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                        init.push(format!("    MOVLW {lit}({f})"));
                    } else {
                        init.push(format!("    MOVLW 0x{b:02X}"));
                    }
                    init.push(format!("    MOVWF 0x{:02X}", base + i as u16));
                }
            }
        }
        let mut start_block: Vec<String> = vec![
            "__start:".to_string(),
            "    MOVLW PAGE(main)".to_string(),
            "    MOVWF PCLATH".to_string(),
        ];
        start_block.extend(init);
        start_block.extend([
            "    CALL main".to_string(),
            "    SLEEP".to_string(),
            "".to_string(),
        ]);
        let start_len = start_block.len();
        out.extend(start_block);
        locs.extend(std::iter::repeat(None).take(start_len));
    }
    // Resolves pointer chains eagerly to folded bases keyed like locals.
    // Seeds byval, sret, and alloca bases first. Shares the fold with the
    // sibling backend through `iselcore`.
    let resolved = resolve_pointers(m);
    // Keeps labels file-scoped with one module counter.
    // Pass A emits every body with all restores, then assigns pages.
    // Forward targets need computed sizes first, so one pass cannot do
    // both. Emits the ISR first at the vector, then module order.
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
                w_holds: None,
                cur_loc: None,
                out: Vec::new(),
                locs: Vec::new(),
            };
            emit_func_body(&mut g, f);
            bodies.push((f.name.clone(), word_size(&g.out)));
            body_texts.push(g.out.join("\n"));
        }
    }
    // Packs post-banking sizes, so BANKSEL growth cannot push a tail
    // across a boundary (epic-cc#17). Counts stay placement-independent,
    // and pass-B elision only shrinks. Module asm precedes `__start` in
    // the page-0 base (epic-cc#207).
    let modasm_lines: Vec<String> = m
        .module_asm
        .iter()
        .flat_map(|e| e.split('\n'))
        .map(|l| l.to_string())
        .collect();
    let mut measure: Vec<String> = vec![
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    measure.extend(modasm_lines.iter().cloned());
    if !has_isr {
        let mut init: Vec<String> = Vec::new();
        for g in &m.globals {
            if g.is_const && addrs.contains_key(&g.name) {
                let base = addrs[&g.name];
                for (i, b) in g.bytes.iter().enumerate() {
                    // Materializes function address fields as link-time
                    // literals (epic-cc#154).
                    if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                        let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                        init.push(format!("    MOVLW {lit}({f})"));
                    } else {
                        init.push(format!("    MOVLW 0x{b:02X}"));
                    }
                    init.push(format!("    MOVWF 0x{:02X}", base + i as u16));
                }
            }
        }
        let mut blk: Vec<String> = vec![
            "__start:".to_string(),
            "    MOVLW PAGE(main)".to_string(),
            "    MOVWF PCLATH".to_string(),
        ];
        blk.extend(init);
        blk.extend([
            "    CALL main".to_string(),
            "    SLEEP".to_string(),
            "".to_string(),
        ]);
        measure.extend(blk);
    }
    for (i, (name, _)) in bodies.iter().enumerate() {
        measure.push(body_texts[i].clone());
        if has_isr && name == isr_names[0] {
            let mut init: Vec<String> = Vec::new();
            for g in &m.globals {
                if g.is_const && addrs.contains_key(&g.name) {
                    let base = addrs[&g.name];
                    for (idx, b) in g.bytes.iter().enumerate() {
                        // Materializes function address fields as link-time
                        // literals (epic-cc#154).
                        if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == idx) {
                            let lit = if idx % 2 == 0 { "LOW" } else { "HIGH" };
                            init.push(format!("    MOVLW {lit}({f})"));
                        } else {
                            init.push(format!("    MOVLW 0x{b:02X}"));
                        }
                        init.push(format!("    MOVWF 0x{:02X}", base + idx as u16));
                    }
                }
            }
            let mut blk: Vec<String> = vec![
                "__start:".to_string(),
                "    MOVLW PAGE(main)".to_string(),
                "    MOVWF PCLATH".to_string(),
            ];
            blk.extend(init);
            blk.extend([
                "    CALL main".to_string(),
                "    SLEEP".to_string(),
                "".to_string(),
            ]);
            measure.extend(blk);
        }
    }
    let banked = banking::assign_banks(device, &measure.join("\n"));
    // Measures post-banking extents label to label with assembler
    // semantics. Block labels stay inside the enclosing function, matching
    // the page-fit check.
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
    // Packs first-fit into the lowest page with room, reusing tails to use
    // fewer pages. Starts after the page-0 prefix with `__start` reachable
    // and the ISR pinned at the vector (epic-cc#207).
    let mut pages: HashMap<String, usize> = HashMap::new();
    let mut pads: HashMap<String, usize> = HashMap::new();
    let init_words = start_init_words(m, addrs);
    let mut page_next: Vec<usize> = vec![if has_isr {
        4
    } else {
        5 + word_size(&modasm_lines) + init_words
    }];
    for (name, _) in &bodies {
        let size = post[name];
        if has_isr && name == isr_names[0] {
            assert!(
                4 + size + 4 + init_words <= 0x800,
                "isel: isr @{name} of {size} words does not fit page 0 (0x004-0x7FF) with room for the reset __start"
            );
            pages.insert(name.clone(), 0);
            pads.insert(name.clone(), 4);
            // Counts the ISR body plus the following `__start` and init in
            // the page-0 budget (epic-cc#207).
            page_next[0] = 4 + size + 4 + init_words;
        } else {
            if size > 0x800 {
                panic!("isel: function @{name} of {size} words exceeds a 2048-word page (0x800)");
            }
            // Places each function in the lowest fitting page tail.
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
                    // Opens the next page when no tail fits. Enforces the
                    // device flash bound there: overflow panics.
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
            // Anchors page-aligned starts with `.org`: without it, pass-B
            // shrinkage slides the function below the boundary and its label
            // and tail disagree on the page.
            if start & 0x7FF == 0 {
                pads.insert(name.clone(), start);
            }
        }
    }
    // Maps const readers to post-call pages. Pins the section start, so the
    // map holds in the final text.
    let mut consts: Vec<&ir::Global> = m
        .globals
        .iter()
        .filter(|g| g.is_const && !addrs.contains_key(&g.name))
        .collect();
    consts.sort_by_key(|g| g.name.clone());
    let table_start = page_next
        .last()
        .copied()
        .unwrap_or(if has_isr { 4 } else { 5 });
    for (entry, page) in reader_pages(&consts, table_start) {
        pages.insert(entry, page);
    }
    // Pass B emits with pages known in page order. Skips same-page restores
    // under pinned bases. Orders by page to keep addresses monotonic within
    // each page.
    let mut page_order: Vec<Vec<(&ir::Func, &str)>> = Vec::new();
    for (f, (name, _)) in order.iter().zip(&bodies) {
        let page = pages[name];
        while page_order.len() <= page {
            page_order.push(Vec::new());
        }
        page_order[page].push((*f, name.as_str()));
    }
    let section_start = {
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
                    w_holds: None,
                    cur_loc: None,
                    out: Vec::new(),
                    locs: Vec::new(),
                };
                emit_func_body(&mut g, f);
                if let Some(pad) = pads.get(*name) {
                    out.push(format!("    org 0x{pad:04X}"));
                    locs.push(None);
                    addr_b = *pad;
                }
                addr_b += word_size(&g.out);
                out.extend(g.out);
                locs.extend(g.locs);
                if f.isr {
                    // Moves `__start` after the ISR within page 0, keeping it
                    // in reset reach.
                    let mut init: Vec<String> = Vec::new();
                    for g in &m.globals {
                        if g.is_const && addrs.contains_key(&g.name) {
                            let base = addrs[&g.name];
                            for (i, b) in g.bytes.iter().enumerate() {
                                init.push(format!("    MOVLW 0x{b:02X}"));
                                init.push(format!("    MOVWF 0x{:02X}", base + i as u16));
                            }
                        }
                    }
                    let mut blk: Vec<String> = vec![
                        "__start:".to_string(),
                        "    MOVLW PAGE(main)".to_string(),
                        "    MOVWF PCLATH".to_string(),
                    ];
                    let init_len = init.len();
                    blk.extend(init);
                    blk.extend([
                        "    CALL main".to_string(),
                        "    SLEEP".to_string(),
                        "".to_string(),
                    ]);
                    let blk_len = blk.len();
                    out.extend(blk);
                    locs.extend(std::iter::repeat(None).take(blk_len));
                    addr_b += 4 + init_len;
                }
            }
        }
        // Pins the table section when elision moves a reader base across a
        // page: re-anchors to the pass-A start, so the restore map holds.
        // Window fit runs on the final banked text with readers included.
        let mut start = addr_b;
        if !consts.is_empty() {
            // Measures with placeholder readers, so banking sees the same
            // operand banks as the real text. Stops at the first reader.
            let mut code_text = out.join("\n");
            for g in &consts {
                code_text.push_str("\n__read_");
                code_text.push_str(&g.name);
                code_text.push_str(
                    ":\n    MOVWF 0x70\n    MOVLW 0x00\n    MOVWF PCLATH\n    MOVF 0x70, W\n    ADDLW 0x00\n    MOVWF PCL\n    RETLW 0x00",
                );
            }
            let banked = banking::assign_banks(device, &code_text);
            let peeped = peephole::optimize(&banked);
            start = measure_end_org(&peeped);
            // Compares pass-A and actual reader pages at the banked address:
            // growth can cross a page, so the check runs there and pins on
            // drift.
            let pages_a = reader_pages(&consts, table_start);
            let pages_b = reader_pages(&consts, start);
            let drift = pages_a
                .iter()
                .zip(&pages_b)
                .any(|((_, pa), (_, pb))| pa != pb);
            if drift {
                out.push(format!("    org 0x{table_start:04X}"));
                locs.push(None);
                start = table_start;
            }
        }
        start
    };
    // Emits const globals as RETLW tables after functions. Sets the window
    // before each computed jump. Aligns large tables by chunk. Guards label
    // uniqueness across bases, readers, and chunks. Oversize tables panic:
    // the index width is closed.
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
            // Matches chunk counts with the reader: small tables emit one,
            // large tables emit ceiling division with at least two.
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
    let mut addr = section_start;
    for g in consts {
        assert!(
            !g.bytes.is_empty(),
            "isel: const @{} has no table bytes",
            g.name
        );
        let size = g.bytes.len();
        // Keeps the two-chunk shape for 256-byte tables: the dispatch tests
        // the chunk bit, so chunk 1 stays present. Larger tables divide by
        // chunk size.
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
        // Stashes the index across the window set, then jumps through the
        // computed address.
        let reader = |out: &mut Vec<String>, locs: &mut Vec<Option<SrcLoc>>, base: &str| {
            out.push(format!("    MOVWF 0x{:02X}", scratch));
            locs.push(None);
            out.push(format!("    MOVLW HIGH({base})"));
            locs.push(None);
            out.push("    MOVWF PCLATH".to_string());
            locs.push(None);
            out.push(format!("    MOVF 0x{:02X}, W", scratch));
            locs.push(None);
            out.push(format!("    ADDLW LOW({base})"));
            locs.push(None);
            out.push("    MOVWF PCL".to_string());
            locs.push(None);
        };
        if size >= 256 {
            // Lays out chunked tables with aligned bases and trailing
            // readers. Keeps the chunk-1 reader name stable. Routes empty
            // tail chunks as unreachable.
            out.push(format!("__read_{}:", g.name));
            locs.push(None);
            reader(&mut out, &mut locs, &g.name);
            out.push("    .align 256".to_string());
            locs.push(None);
            out.push(format!("    .table {} {size}", g.name));
            locs.push(None);
            out.push(format!("{}:", g.name));
            locs.push(None);
            for (i, b) in g.bytes[..256].iter().enumerate() {
                if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                    let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                    out.push(format!("    RETLW {lit}({f})"));
                } else {
                    out.push(format!("    RETLW 0x{b:02X}"));
                }
                locs.push(None);
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
                locs.push(None);
                for (i, b) in g.bytes[start..end.min(size)].iter().enumerate() {
                    let abs = start + i;
                    if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == abs) {
                        let lit = if abs % 2 == 0 { "LOW" } else { "HIGH" };
                        out.push(format!("    RETLW {lit}({f})"));
                    } else {
                        out.push(format!("    RETLW 0x{b:02X}"));
                    }
                    locs.push(None);
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
                locs.push(None);
                reader(&mut out, &mut locs, &chunk_label);
            }
        } else {
            // Aligns a small base that would cross its window, so the window
            // check holds by construction (epic-cc#138). Skips alignment
            // when the base already fits.
            out.push(format!("__read_{}:", g.name));
            locs.push(None);
            reader(&mut out, &mut locs, &g.name);
            let base = window_align(addr + 6, size);
            if base != addr + 6 {
                out.push("    .align 256".to_string());
                locs.push(None);
            }
            out.push(format!("    .table {} {size}", g.name));
            locs.push(None);
            out.push(format!("{}:", g.name));
            locs.push(None);
            for (i, b) in g.bytes[..size].iter().enumerate() {
                // Materializes function address fields as link-time literals
                // (epic-cc#154).
                if let Some((_, f)) = g.refs.iter().find(|(o, _)| *o == i) {
                    let lit = if i % 2 == 0 { "LOW" } else { "HIGH" };
                    out.push(format!("    RETLW {lit}({f})"));
                } else {
                    out.push(format!("    RETLW 0x{b:02X}"));
                }
                locs.push(None);
            }
        }
        // Tracks table words to keep the running address consistent.
        // Tables do not move function placement, already decided.
        addr += 6; // reader entry (MOVWF/MOVLW/MOVWF/MOVF/ADDLW/MOVWF PCL)
        if size >= 256 {
            addr = (addr + 255) & !255; // `.align 256`
            addr += 256; // chunk 0 RETLWs
            addr += size - 256; // chunks 1.. RETLWs
            addr += 6 * (n_chunks - 1); // chunk reader entries
        } else {
            // Folds alignment into the running address when the natural base
            // would cross its window.
            addr = window_align(addr, size) + size;
        }
        out.push("".to_string());
        locs.push(None);
    }
    out.push("    end".to_string());
    locs.push(None);
    (out.join("\n"), locs)
}

/// Re-exports the address-map parser from `iselcore`. Keeps the backend
/// binary working without duplicating the text format.
pub use iselcore::parse_map;
