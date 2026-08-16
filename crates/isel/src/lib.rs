//! `isel` — instruction selection for the integer spine.
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
//! (RAM or const-flash), `Base::Slot(name, indirect)` — a byval param copy,
//! an alloca, or an sret slot holding a target address (indirect). A
//! constant offset (no terms) reads/writes the plain file register; dynamic
//! terms set `FSR` to `base + k + Σ s×%r` (single scale-1 term keeps the M5
//! `MOVF %r,W; ADDLW base+k; MOVWF FSR` fast path; general sums accumulate
//! in the fixed scratch byte); an indirect base takes the target address
//! from the slot's contents. Pointers into const (flash) globals load via
//! `CALL __read_<name>` — a RETLW table emitted after the functions — and a
//! store through a const base panics (ROM is not writable). `memcpy`
//! lowers to a byte loop of the same pointer machinery; `alloca` is virtual
//! like `gep` (the slot is sized by alloc). Static FSR bases reach all four
//! GPR banks via the IRP bit (M9): every FSR setup emits `BCF/BSF STATUS, 7`
//! (IRP = base bit 8) first, then loads FSR with `(base + k + off) & 0xFF`.
//! The FSR-accessed object must fit entirely inside one of the four GPR
//! windows `[0x20,0x80)` `[0xA0,0xF0)` `[0x120,0x170)` `[0x1A0,0x1F0)` —
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

use ir::{BinOp, Gep, GepBase, Inst, Module, Ty, Val};
use std::collections::HashMap;

/// Map key for a local value: `{func}::{name}` (IR value names without `%`).
/// Matches the keys `alloc` emits in its overlay layout, so a callee's param
/// slots and the caller's live slots never collide across CALL boundaries.
fn ssa_key(func: &str, name: &str) -> String {
    format!("{func}::{name}")
}

/// The runtime routine names legalize injects for mul/div/rem/shift. All
/// twenty-four have recipe bodies (Task 3: the i32 mul/div/rem + shifts;
/// M8: the i8/i16 set). An injected routine's entry block holds only a
/// scratch alloca, so emitting it as-is would produce an empty label that
/// silently falls through into the next function — a routine name with no
/// recipe yet must panic loudly instead.
const ROUTINE_NAMES: [&str; 24] = [
    "__mul_u8",
    "__mul_u16",
    "__mul_u32",
    "__udiv_u8",
    "__urem_u8",
    "__udiv_u16",
    "__urem_u16",
    "__udiv_u32",
    "__urem_u32",
    "__sdiv_i8",
    "__srem_i8",
    "__sdiv_i16",
    "__srem_i16",
    "__sdiv_i32",
    "__srem_i32",
    "__shl_u8",
    "__lshr_u8",
    "__ashr_i8",
    "__shl_u16",
    "__lshr_u16",
    "__ashr_i16",
    "__shl_u32",
    "__lshr_u32",
    "__ashr_i32",
];

fn is_routine_name(name: &str) -> bool {
    ROUTINE_NAMES.contains(&name)
}

/// The byte address of a literal-pointer operand (`"0x<K>"` — the
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

/// A resolved GEP base: a named global (`@g`) or a local slot. A slot may
/// be *indirect* (an sret param): it holds the target address, so FSR is
/// taken from its contents rather than the slot itself being the base.
#[derive(Clone, Debug)]
enum Base {
    Global(String),
    Slot(String, bool),
}

/// The four GPR windows reachable through FSR+IRP: bank 0 `[0x20,0x80)`,
/// bank 1 `[0xA0,0xF0)`, bank 2 `[0x120,0x170)`, bank 3 `[0x1A0,0x1F0)`
/// (the common region 0x70-0x7F sits inside the first window). The SFR
/// holes 0x80-0x9F and 0x170-0x19F are not addressable GPR, so an object
/// that does not fit entirely inside its base's window would silently
/// mis-address through INDF — it panics loudly instead.
///
/// Returns `(irp, base_lo)`: `IRP = bit 8 of the base` (STATUS bit 7) and
/// `FSR = base & 0xFF` (0x120 -> 0x20, 0x1A0 -> 0xA0).
fn fsr_window(base_addr: u16, span: u16) -> (bool, u8) {
    let win_end = match base_addr {
        0x20..=0x7F => 0x80,
        0xA0..=0xEF => 0xF0,
        0x120..=0x16F => 0x170,
        0x1A0..=0x1EF => 0x1F0,
        _ => panic!(
            "isel: FSR base 0x{base_addr:03X} outside GPR space (windows [0x20,0x80) [0xA0,0xF0) [0x120,0x170) [0x1A0,0x1F0))"
        ),
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
    /// Every pointer reg in the module, keyed `{func}::{reg}`, resolved to
    /// its folded `(base, k, terms)` — GEP chains fully collapsed (base
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
    /// sizes drive the greedy assignment); `Some` in pass B, where a
    /// same-page restore is skipped.
    page_of: Option<&'m HashMap<String, usize>>,
    out: Vec<String>,
}

impl<'m> Gen<'m> {
    fn emit(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    /// The M11 restore pair — `MOVLW PAGE(<cur_func>); MOVWF PCLATH` — right
    /// after a CALL. Skipped when the called target runs in the current
    /// function's own page: the set already wrote that page and nothing since
    /// changed PCLATH<4:3> (a function callee restores itself; a const
    /// reader leaves `HIGH(<base>)` whose bits 4:3 are the table base's page,
    /// equal to the target's page). In pass A (`page_of` is `None`) the
    /// pages are not known yet, so the restore is always emitted — pass A's
    /// sizes (with every restore) drive the greedy page assignment, and the
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
    fn slot_addr(&self, func: &str, name: &str) -> u16 {
        *self
            .addrs
            .get(&ssa_key(func, name))
            .unwrap_or_else(|| panic!("isel: no slot for {func}::{name}"))
    }

    /// Resolve an operand value to its base byte address (lo for multi-byte).
    fn val_addr(&self, v: &Val) -> u16 {
        match v {
            Val::Reg(r) => self.slot_addr(self.cur_func, r),
            Val::Global(g) => *self
                .addrs
                .get(g)
                .unwrap_or_else(|| panic!("isel: no address for @{g}")),
            Val::Const(k) => {
                assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
                *k as u16
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

    /// Whether `name` is a const (flash) global — read via RETLW tables.
    fn global_is_const(&self, name: &str) -> bool {
        self.m
            .globals
            .iter()
            .find(|g| g.name == name)
            .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
            .is_const
    }

    /// The byte size of a global — a const table's size selects the reader
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
    /// index is the 16-bit reg clang zexts — reading the hi slot of a
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
                    _ => continue,
                };
                return w;
            }
        }
        panic!("isel: no definition of %{name} in {}", self.cur_func);
    }

    /// The byte span of a resolved FSR base — the whole object a pointer
    /// into it can legally touch (the runtime terms are bounded by span−1).
    /// `Base::Global` spans its `Global.size`; `Base::Slot` spans the byval
    /// param's `width` or the alloca's `size` in the current function. A
    /// missing object panics loudly.
    fn object_span(&self, base: &Base) -> u16 {
        match base {
            Base::Global(name) => self
                .m
                .globals
                .iter()
                .find(|g| g.name == *name)
                .unwrap_or_else(|| panic!("isel: unknown global @{name}"))
                .size as u16,
            Base::Slot(sname, _) => {
                let f = self
                    .m
                    .funcs
                    .iter()
                    .find(|f| f.name == self.cur_func)
                    .unwrap_or_else(|| {
                        panic!("isel: no span for slot {sname}: unknown function {}", self.cur_func)
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

    /// The resolved `(base, k, terms)` for a pointer reg `%r` — a GEP dst,
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
                            // known — a plain file-register access, no FSR.
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
                        let sa = self.slot_addr(self.cur_func, sname);
                        if !indirect && terms.is_empty() {
                            Addr::Direct(sa + u16::from(k) + u16::from(byte_off))
                        } else if !indirect {
                            let span = self.object_span(&base);
                            self.emit_fsr_to(sa, k, &terms, byte_off, span);
                            Addr::Indirect
                        } else {
                            self.emit_fsr_indirect(sa, k, &terms, byte_off);
                            Addr::Indirect
                        }
                    }
                }
            }
            Val::Const(_) => panic!("isel: pointer operand must be a register or global"),
        }
    }

    /// `W = RAM[ptr + byte_off]` — one byte of a pointer load or a memcpy
    /// source. Direct bases read the plain file register; dynamic bases set
    /// FSR first and read INDF; a const (flash) base reads via
    /// `CALL __read_<name>` (the RETLW table leaves the byte in W). A table
    /// larger than 255 bytes takes the 16-bit index path — the caller
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

    /// `RAM[ptr + byte_off] = W` — the store side of a byte access (memcpy
    /// destinations; `emit_ptr_store_byte` composes a val load before it).
    fn emit_ptr_store_w(&mut self, ptr: &Val, byte_off: u8) {
        match self.emit_ptr_setup(ptr, byte_off) {
            Addr::Direct(a) => self.emit(format!("    MOVWF 0x{a:02X}")),
            Addr::Indirect => self.emit("    MOVWF INDF".to_string()),
        }
    }

    /// `RAM[ptr + byte_off] = byte byte_off of val`.
    fn emit_ptr_store_byte(&mut self, ptr: &Val, byte_off: u8, val: &Val) {
        // The address setup comes first — its FSR/scratch computation
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

    /// `FSR = base_addr + k + byte_off + Σ scale×%reg`, for an object of
    /// `span` bytes at `base_addr`. The window check runs first: the whole
    /// object must fit inside its base's GPR window (an unrepresentable
    /// cross-hole address would silently mis-address through INDF, so it
    /// panics loudly). IRP is then set on EVERY FSR setup — a prior
    /// bank-2/3 access leaves STATUS bit 7 = 1, so skipping the set on a
    /// bank-0/1 base would mis-address into bank 2/3. A single scale-1
    /// term keeps the M5 fast shape (`MOVF %r,W; ADDLW lit; MOVWF FSR`);
    /// general sums accumulate in the fixed scratch byte first. The ADDLW
    /// literal is `(base_addr + k + byte_off) & 0xFF` — FSR holds the low
    /// byte; IRP carries bit 8.
    fn emit_fsr_to(&mut self, base_addr: u16, k: u8, terms: &[(u8, String)], byte_off: u8, span: u16) {
        let (irp, base_lo) = fsr_window(base_addr, span);
        let lit = (u16::from(base_lo) + u16::from(k) + u16::from(byte_off)) & 0xFF;
        self.emit(if irp {
            "    BSF STATUS, 7".to_string()
        } else {
            "    BCF STATUS, 7".to_string()
        });
        match terms {
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone()));
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
    /// address's bit 8 — 1 -> IRP=1 (banks 2/3), 0 -> IRP=0 (banks 0/1).
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
    /// ADDWF — it MUST be reloaded before each repetition or a scaled term
    /// accumulates 2×scratch + %r (silent wrong-address miscompile).
    fn emit_accum_terms(&mut self, terms: &[(u8, String)]) {
        self.emit("    MOVLW 0x00".to_string());
        self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
        for (scale, r) in terms {
            let a = self.val_addr(&Val::Reg(r.clone()));
            for _ in 0..*scale {
                self.emit(format!("    MOVF 0x{a:02X}, W"));
                self.emit(format!("    ADDWF 0x{:02X}, W", self.scratch));
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch));
            }
        }
    }

    /// `W = k + byte_off + Σ scale×%reg` — the byte index into a const
    /// (flash) table before `CALL __read_<name>`. A single scale-1 term
    /// keeps the M5 `MOVF %r,W` shape (ADDLW only when k + off is nonzero);
    /// general sums accumulate in scratch.
    fn emit_ptr_index_w(&mut self, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(kk <= 0xFF, "isel: const index k {k} + off {byte_off} out of byte range");
        match terms {
            [] => self.emit(format!("    MOVLW 0x{kk:02X}")),
            [(1, r)] => {
                let a = self.val_addr(&Val::Reg(r.clone()));
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

    /// Large const table (> 255 bytes) read. The 16-bit GEP index — a
    /// single scale-1 term (clang: `zext i8 %idx to i16`, then `gep @t +k
    /// +1*%i`) — splits into the in-chunk index (W) and the chunk bit (hi
    /// temp, fixed scratch 0x70): `MOVF r_lo,W; ADDLW k+off` sets C on the
    /// carry into bit 8, and `BTFSC STATUS,0; ADDLW 0x01` folds it into the
    /// hi byte, so e.g. idx 0x00F0 + k 0x20 -> in-chunk 0x10, hi 1. W is
    /// restored from the lo temp (0x71, retval_lo — no live retval at a
    /// const read) and bit 0 of the hi temp selects `__read_<name>` (chunk
    /// 0) or `__read_<name>_hi` (chunk 1). The read leaves the byte in W,
    /// exactly like the small-table path. Const-only and multi-term
    /// 16-bit indices into large tables panic loudly for now.
    fn emit_const_read_large(&mut self, name: &str, k: u8, terms: &[(u8, String)], byte_off: u8) {
        let kk = u16::from(k) + u16::from(byte_off);
        assert!(
            kk <= 0xFF,
            "isel: const index k {k} + off {byte_off} out of byte range"
        );
        match terms {
            [(1, r)] => {
                assert_eq!(
                    self.reg_bytes(r),
                    2,
                    "isel: large-table index %{r} must be a 16-bit reg (clang zexts the byte index)"
                );
                let a_lo = self.val_addr(&Val::Reg(r.clone()));
                let l_hi = self.fresh_label();
                let l_done = self.fresh_label();
                // W = lo + k + off; C = carry into bit 8.
                self.emit(format!("    MOVF 0x{a_lo:02X}, W"));
                self.emit(format!("    ADDLW 0x{kk:02X}"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo)); // lo temp
                // W = hi + carry.
                self.emit(format!("    MOVF 0x{:02X}, W", a_lo + 1));
                self.emit("    BTFSC STATUS, 0".to_string());
                self.emit("    ADDLW 0x01".to_string());
                self.emit(format!("    MOVWF 0x{:02X}", self.scratch)); // hi temp (chunk bit)
                // W = in-chunk index; bit 0 of the hi temp selects the entry.
                // The GOTO below is intra-function — it must run with the
                // CALLER's page, so each reader CALL's set comes after it.
                // The set's MOVLW clobbers W (the index), which is then
                // reloaded from the lo temp (0x71 — no live retval at a
                // const read); the returned byte survives the restore via
                // the hi temp (0x70, dead after the chunk-bit test).
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
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    MOVF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
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
                let a = self.val_addr(&Val::Reg(r.clone()));
                self.emit(format!("    XORWF 0x{:02X}, W", a + u16::from(idx)));
            }
            Val::Global(g) => {
                let a = self.val_addr(&Val::Global(g.clone()));
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
    /// maps signed order onto unsigned order — signed(a >= b) ==
    /// unsigned((a ^ 0x80) >= (b ^ 0x80)) — so one flag recipe serves both.
    fn emit_load_cmp_byte(&mut self, v: &Val, i: u8, signed: bool, high: u8) {
        match v {
            Val::Const(k) => {
                let byte = ((k >> (i as u32 * 8)) & 0xFF) as u8;
                let b = if signed && i == high { byte ^ 0x80 } else { byte };
                self.emit(format!("    MOVLW 0x{b:02X}"));
            }
            _ => {
                let addr = self.val_addr(v) + u16::from(i);
                if signed && i == high {
                    self.emit("    MOVLW 0x80".to_string());
                    self.emit(format!("    XORWF 0x{addr:02X}, W"));
                } else {
                    self.emit(format!("    MOVF 0x{addr:02X}, W"));
                }
            }
        }
    }

    /// Set C = (a >= b) — unsigned or signed (sign-bit complement). For i8
    /// the SUBWF/SUBLW also leaves Z = (a == b); for i16 the borrow chain's
    /// final Z is only a byte-level flag, so predicates needing equality
    /// append `emit_cmp_eq` (which preserves C). A const RHS becomes the
    /// MOVLW/SUBWF subtrahend; a const LHS uses SUBLW (k - W) since a const
    /// can never be read as a file register.
    fn emit_cmp_c(&mut self, a: &Val, b: &Val, ty: Ty, signed: bool) {
        let n = ty.bytes();
        let high = n - 1;
        match (a, b) {
            (Val::Const(_), Val::Const(_)) => panic!("isel: constant folding not implemented"),
            (Val::Const(k), _) => {
                if n > 2 {
                    // The 4-byte chain needs the wrap-correct borrow folds
                    // (the naive ADDLW 1 fold corrupts the borrow-out at
                    // b_i = 0xFF + borrow-in); the i8/i16 paths below stay
                    // byte-identical.
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
                if n > 2 {
                    self.emit_cmp_c_file_lhs_wide(a, b, n, high, signed);
                    return;
                }
                let aa = self.val_addr(a);
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
                    if use_scratch && n == 1 { self.scratch } else { aa }
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

    /// The wide (n > 2, i.e. i32) borrow chain for `C = (a >= b)` with a
    /// file-LHS `a`. The 4-byte chain's intermediate borrow-outs are
    /// load-bearing, and the 2-byte chain's `ADDLW 1` fold corrupts the
    /// borrow-out exactly when the folded subtrahend wraps (b_i = 0xFF +
    /// borrow-in = 0x100): the SUBWF then sees W = 0 and leaves
    /// C = (a_i >= 0) = 1 — a false "no borrow" that mis-compares every
    /// higher byte. Folding via INCFSZ's skip keeps C = borrow-in, the true
    /// borrow-out, and the skipped SUBWF's garbage W result is discarded
    /// (a cmp leaves only flags; a and b are never written, so INCFSZ can
    /// fold directly on the operand byte). The signed sign-complement
    /// applies to the HIGH byte only: the a-side is XORed 0x80 into the
    /// scratch (the SUBWF file operand), and the b-side is complemented
    /// into the 0x71 temp and folded *complemented* via INCFSZ's skip —
    /// the complemented fold wraps at b_hi ^ 0x80 = 0xFF (b_hi = 0x7F +
    /// borrow), where the skip keeps C = borrow-in = 0, the true
    /// borrow-out. A fold on the uncomplemented byte would repair only the
    /// b_hi = 0xFF wrap; b_hi = 0x7F + borrow would wrap invisibly and
    /// corrupt the final C.
    fn emit_cmp_c_file_lhs_wide(&mut self, a: &Val, b: &Val, n: u8, high: u8, signed: bool) {
        let aa = self.val_addr(a);
        // Byte 0 has no borrow-in; a single SUBWF leaves C exact.
        self.emit_load_cmp_byte(b, 0, signed, high);
        self.emit(format!("    SUBWF 0x{aa:02X}, W"));
        for i in 1..n {
            if signed && i == high {
                // Both sides are complemented at the high byte. The b-side
                // is folded COMPLEMENTED via INCFSZ's skip (0x71 as a
                // second temp — no live retval during a compare), because
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
                        let addr = self.val_addr(b) + u16::from(high);
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
                        let addr = self.val_addr(b) + u16::from(i);
                        self.emit("    BTFSS STATUS, 0 ; C".to_string());
                        self.emit(format!("    INCFSZ 0x{addr:02X}, W"));
                    }
                }
                self.emit(format!("    SUBWF 0x{:02X}, W", aa + u16::from(i)));
            }
        }
    }

    /// The wide const-LHS (SUBLW) borrow chain: W holds the b byte
    /// (+ borrow), SUBLW subtracts it from the const byte. Same
    /// wrap-correct folds as `emit_cmp_c_file_lhs_wide`; the signed
    /// high byte's literal is pre-complemented (folded into the SUBLW
    /// operand) and the b-side is complemented into the 0x71 temp and
    /// folded COMPLEMENTED via INCFSZ's skip — the complemented fold
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
                // emit_cmp_c_file_lhs_wide — the complemented fold wraps at
                // b_hi = 0x7F + borrow, where the skip keeps the true
                // borrow-out).
                let addr = self.val_addr(b) + u16::from(high);
                self.emit("    MOVLW 0x80".to_string());
                self.emit(format!("    XORWF 0x{addr:02X}, W"));
                self.emit(format!("    MOVWF 0x{:02X}", self.retval_lo));
                self.emit("    BTFSS STATUS, 0 ; C".to_string());
                self.emit(format!("    INCFSZ 0x{:02X}, W", self.retval_lo));
                let kb = ((k >> (high as u32 * 8)) & 0xFF) as u8 ^ 0x80;
                self.emit(format!("    SUBLW 0x{kb:02X}"));
            } else {
                let addr = self.val_addr(b) + u16::from(i);
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
                let ca = self.val_addr(&Val::Reg(r.clone()));
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
        let da = self.slot_addr(self.cur_func, dst);
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
            Val::Reg(r) => self.val_addr(&Val::Reg(r.clone())),
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
        let ra = self.val_addr(&Val::Reg(reg));
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
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
        let ra = self.val_addr(&Val::Reg(reg));
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
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
    /// subtracts it from the minuend — SUBWF f,W always computes f - W, so
    /// `a` is the file operand. A const LHS is rejected by the caller (sub
    /// is not commutative).
    fn emit_sub8(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a);
        match b {
            Val::Const(k) => {
                assert!(*k >= 0 && *k <= 255, "isel: const {k} out of byte range");
                self.emit(format!("    MOVLW 0x{:02X}", *k as u8));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Reg(_) => {
                let bb = self.val_addr(b);
                self.emit(format!("    MOVF 0x{bb:02X}, W"));
                self.emit(format!("    SUBWF 0x{aa:02X}, W"));
                self.emit(format!("    MOVWF 0x{dst:02X}"));
            }
            Val::Global(_) => panic!("isel: sub8 with a global operand"),
        }
    }

    /// `d = a - b` for i16: low byte SUBWF, then the high byte with the
    /// borrow from the low byte folded in — if C is clear (borrow), ADDLW 1
    /// bumps the subtrahend byte before the high SUBWF.
    fn emit_sub16(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a);
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
                let bb = self.val_addr(&Val::Reg(rb.clone()));
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
    /// destination at `a_i` — the correct mod-256 result — with C =
    /// carry-in, the true carry-out. The i16 fold's C would be corrupted at
    /// an intermediate byte (`SUBWF`-style re-derivation gives
    /// C = (a_i >= 0) = 1 there), silently mis-adding every higher byte.
    fn emit_add32(&mut self, a: &Val, b: &Val, dst: u16) {
        let (reg, other) = match (a, b) {
            (Val::Reg(r), o) => (r.clone(), o),
            (o, Val::Reg(r)) => (r.clone(), o),
            _ => panic!("isel: add32 needs a register operand"),
        };
        let ra = self.val_addr(&Val::Reg(reg));
        // Byte 0: no carry-in; the ADDWF's C is exact.
        match other {
            Val::Reg(rb) => {
                let bb = self.val_addr(&Val::Reg(rb.clone()));
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
    /// destination at `a_i` — the correct mod-256 result — with C =
    /// borrow-in = 0, the true borrow-out. The i16 fold's C would be
    /// corrupted at an intermediate byte (C = (a_i >= 0) = 1), silently
    /// mis-subtracting every higher byte.
    fn emit_sub32(&mut self, a: &Val, b: &Val, dst: u16) {
        let aa = self.val_addr(a);
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
                let bb = self.val_addr(&Val::Reg(rb.clone()));
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
    /// (`retval_lo` .. `retval_lo + bytes - 1` — 0x71-0x74 for i32) into
    /// `dst`. Void calls skip the retval copy. Mirrors spike emit_call.
    fn emit_call(&mut self, dst: &Option<String>, ty: Option<Ty>, func: &str, args: &[ir::CallArg]) {
        let callee = self
            .m
            .funcs
            .iter()
            .find(|f| f.name == func)
            .unwrap_or_else(|| panic!("isel: call to unknown function @{func}"));
        for (i, arg) in args.iter().enumerate() {
            let pname = &callee.params[i].name;
            let pa = self.slot_addr(func, pname);
            if let Some(size) = arg.byval {
                // byval: copy `size` bytes from the arg's pointer (global /
                // alloca slot / GEP reg) into the callee's param slot — the
                // param slot IS the callee's struct copy (Slot(name, false)),
                // byte by byte through the shared pointer machinery.
                assert_eq!(
                    size,
                    callee.params[i].byval.expect("isel: byval arg for a non-byval param"),
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
                // object must fit entirely inside one GPR window — a span
                // crossing an SFR hole would silently mis-address (the same
                // loud rule as static FSR bases). The MOVLW LOW/HIGH store
                // emits both address bytes unchanged.
                assert!(
                    callee.params[i].sret,
                    "isel: sret arg for a non-sret param"
                );
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
                            Base::Slot(sname, false) => self.slot_addr(self.cur_func, sname),
                            Base::Slot(_, true) => {
                                panic!("isel: sret target cannot be an indirect (sret) slot")
                            }
                        };
                        let span = self.object_span(&base);
                        (addr, span)
                    }
                    Val::Const(_) => panic!("isel: sret target must be a global or an alloca slot"),
                };
                fsr_window(addr, span);
                self.emit(format!("    MOVLW 0x{:02X}", (addr & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa));
                self.emit(format!("    MOVLW 0x{:02X}", ((addr >> 8) & 0xFF) as u8));
                self.emit(format!("    MOVWF 0x{:02X}", pa + 1));
            } else {
                // Scalar args: unchanged.
                let aty = arg.ty.expect("isel: scalar call arg must carry a type");
                self.emit_move_val_to_slot(&arg.val, aty, pa);
            }
        }
        // M11 PCLATH discipline: every CALL runs with PCLATH<4:3> = the
        // target's page. The set's MOVLW clobbers W, so it must come AFTER
        // the last arg copy (which uses W) and immediately before the CALL;
        // the caller's own page is restored right after — unless the target
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
            let da = self.slot_addr(self.cur_func, d);
            for i in 0..t.bytes() {
                self.emit(format!("    MOVF 0x{:02X}, W", self.retval_lo + u16::from(i)));
                self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
            }
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Load(l) => {
                assert!(l.ty != Ty::I1, "isel: only i8/i16 loads supported");
                let dst = self.slot_addr(self.cur_func, &l.dst);
                if let Some(g) = l.ptr.strip_prefix('@') {
                    let src = self.global_addr(g);
                    for k in 0..l.ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", src + u16::from(k)));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                } else if l.ptr.starts_with("0x") {
                    // A literal (SFR) pointer from `inttoptr`: the register
                    // is bank-mirrored (SFR 0x00-0x1F) or common RAM
                    // (0x70-0x7F) — a direct MOVF with no FSR setup and no
                    // BANKSEL. (A literal into a GPR window keeps the plain
                    // direct access too; the banking pass handles it.)
                    let base = literal_ptr_addr(&l.ptr);
                    for k in 0..l.ty.bytes() {
                        self.emit(format!("    MOVF 0x{:02X}, W", base + u16::from(k)));
                        self.emit(format!("    MOVWF 0x{:02X}", dst + u16::from(k)));
                    }
                } else {
                    // A GEP-created pointer: const (flash) bases keep the
                    // RETLW path (i8 only); RAM bases go through the shared
                    // byte machinery (direct or FSR/INDF).
                    let r = l.ptr.strip_prefix('%').unwrap_or_else(|| {
                        panic!("isel: pointer {:?} is not @global, %reg or a literal", l.ptr)
                    });
                    let ptr = Val::Reg(r.to_string());
                    if let (Base::Global(name), _, _) = self.resolved_for(r) {
                        assert!(
                            !self.global_is_const(&name) || l.ty.bytes() == 1,
                            "isel: multi-byte const load not supported"
                        );
                    }
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
                    // A literal (SFR) pointer from `inttoptr` — a direct
                    // MOVWF, no FSR setup, no BANKSEL (bank-mirrored SFR or
                    // common RAM).
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
            Inst::Memcpy(m) => {
                // Byte loop over the same pointer machinery: src[i] -> dst[i].
                // Each byte re-resolves both pointers (dst itself may be a
                // base+k+i expression), exactly like a per-byte load/store.
                for i in 0..m.len {
                    self.emit_ptr_load_byte(&m.src, i);
                    self.emit_ptr_store_w(&m.dst, i);
                }
            }
            Inst::Bin(b) => {
                assert!(b.ty != Ty::I1, "isel: only i8/i16/i32 binops supported");
                let da = self.slot_addr(self.cur_func, &b.dst);
                match (b.op, b.ty) {
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
                                assert!(
                                    *k >= 0 && *k <= 255,
                                    "isel: const {k} out of byte range"
                                );
                                let aa = self.val_addr(a);
                                self.emit(format!("    MOVF 0x{aa:02X}, W"));
                                self.emit(format!("    ADDLW 0x{:02X}", *k as u8));
                                self.emit(format!("    MOVWF 0x{da:02X}"));
                            }
                            _ => {
                                let (aa, bb) = (self.val_addr(a), self.val_addr(b_op));
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
                    // reuse the reg-const lowering (which computes a - k), so
                    // it must fail loudly rather than silently miscompile.
                    (BinOp::Sub, Ty::I8) => {
                        assert!(
                            !matches!(&b.a, Val::Const(_)),
                            "isel: sub with const LHS not supported (not commutative)"
                        );
                        self.emit_sub8(&b.a, &b.b, da);
                    }
                    (BinOp::Sub, Ty::I16) => {
                        assert!(
                            !matches!(&b.a, Val::Const(_)),
                            "isel: sub with const LHS not supported (not commutative)"
                        );
                        self.emit_sub16(&b.a, &b.b, da);
                    }
                    (BinOp::Sub, Ty::I32) => {
                        assert!(
                            !matches!(&b.a, Val::Const(_)),
                            "isel: sub with const LHS not supported (not commutative)"
                        );
                        self.emit_sub32(&b.a, &b.b, da);
                    }
                    // Milestone-8 binops: legalize rewrites every mul/div/rem
                    // into a runtime routine call, so these ops reach isel only
                    // via hand-written IR. Panic loudly — the invariant that a
                    // legalize miss never silently miscompiles.
                    (BinOp::Mul, _) => panic!("isel: mul reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::UDiv, _) => panic!("isel: udiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::URem, _) => panic!("isel: urem reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SDiv, _) => panic!("isel: sdiv reached isel; legalize must rewrite it to a routine call"),
                    (BinOp::SRem, _) => panic!("isel: srem reached isel; legalize must rewrite it to a routine call"),
                    // Milestone-8 shifts: a const count inlines as a fixed
                    // RLF/RRF sequence; k == 0 is a plain copy; k >= width
                    // is LLVM poison and panics loudly. A variable (reg)
                    // count must never reach isel — legalize rewrites it to
                    // the routine call — so one arriving here is a legalize
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
                let da = self.slot_addr(self.cur_func, &f.dst);
                self.emit_move_val_to_slot(&f.val, f.ty, da);
            }
            Inst::Zext(z) => {
                assert!(
                    z.from.bytes() < z.to.bytes(),
                    "isel: zext must widen"
                );
                let da = self.slot_addr(self.cur_func, &z.dst);
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
                let da = self.slot_addr(self.cur_func, &x.dst);
                // Copy the low bytes unchanged.
                for i in 0..x.from.bytes() {
                    self.emit_load_byte(&x.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
                // Fill the high bytes with the source's sign bit: test the
                // MSB of the source's high byte, then MOVLW 0xFF (set) or
                // 0x00 (clear) once and store it into every high byte.
                let src_hi = x.from.bytes() - 1;
                let a = self.val_addr(&x.val);
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
                assert!(
                    t.from.bytes() > t.to.bytes(),
                    "isel: trunc must narrow"
                );
                let da = self.slot_addr(self.cur_func, &t.dst);
                for i in 0..t.to.bytes() {
                    self.emit_load_byte(&t.val, i);
                    self.emit(format!("    MOVWF 0x{:02X}", da + u16::from(i)));
                }
            }
            Inst::Icmp(ic) => {
                let da = self.slot_addr(self.cur_func, &ic.dst);
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

    /// Every recipe slot must sit in bank 0 (≤ 0x7F): 0x80-0xEF maps to
    /// bank 1, and the loops are skip-sensitive (BTFSS + GOTO, DECFSZ +
    /// GOTO) — a BANKSEL the banking pass would insert for a banked slot
    /// would change the skip targets. Loud, documented limitation
    /// (multi-bank runtime routines are a follow-up); the bound matches the
    /// asm encoder's own ≤ 0x7F file-register range.
    fn assert_bank0(&self, addrs: &[u16], routine: &str) {
        for &a in addrs {
            assert!(
                a <= 0x7F,
                "isel: {routine} slot 0x{a:02X} out of bank-0 range (recipe loops are skip-sensitive; a BANKSEL would change skip targets)"
            );
        }
    }

    /// Copy `bytes` bytes from a routine slot into the fixed retval slots
    /// (0x71-0x74) — `emit_call` on the caller side reads them after CALL.
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
    /// bits are discarded — i32 `mul` wraps), shift `bk` right, count 16.
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
    /// borrow chain is exact — no extended-bit special case. C after the
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
    /// offsets. Plain addresses only — the banking pass inserts BANKSELs.
    /// Div-by-zero is LLVM poison: the loop runs (den = 0 ⇒ deterministic
    /// but arbitrary (poison)), any value is legal — no guard, documented. The nine
    /// shift routines (variable count) share `emit_shift_body`.
    fn emit_routine(&mut self) {
        let name = self.cur_func;
        let scr = self.slot_addr(name, "__scr");
        self.emit(format!("{name}:"));
        match name {
            // Variable-count shifts: mask the count to width-1, bounded
            // loop over the val param slot (see emit_shift_body).
            "__shl_u8" | "__lshr_u8" | "__ashr_i8" | "__shl_u16"
            | "__lshr_u16" | "__ashr_i16" | "__shl_u32" | "__lshr_u32"
            | "__ashr_i32" => {
                let (bytes, op) = match name {
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
                let a = self.slot_addr(name, "a");
                let b = self.slot_addr(name, "b");
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
                let a = self.slot_addr(name, "a");
                let b = self.slot_addr(name, "b");
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
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
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
                if name == "__udiv_u8" {
                    self.store_retval(num, 1);
                } else {
                    self.store_retval(rem_lo, 1);
                }
                self.emit("    RETURN".to_string());
            }
            // 16/16 restoring division (16 iterations), the borrow idiom
            // `movf den_hi,w; btfss C; incfsz den_hi,w; subwf rem_hi,f`.
            "__udiv_u16" | "__urem_u16" => {
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
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
                if name == "__udiv_u16" {
                    self.store_retval(num, 2);
                } else {
                    self.store_retval(rem_lo, 2);
                }
                self.emit("    RETURN".to_string());
            }
            // Signed 8-bit wrappers: abs both operands in place in the param
            // slots (unsigned abs — INT_MIN safe), run the unsigned divmod,
            // negate the quotient if the signs differed (bit0) / the
            // remainder if the dividend was negative (bit1).
            "__sdiv_i8" | "__srem_i8" => {
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
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
                if name == "__sdiv_i8" {
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
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
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
                if name == "__sdiv_i16" {
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
            // shifted left one bit per iteration — the shifted-out high
            // bits are DISCARDED, so i32 mul wraps mod 2^32); for each set
            // bit of the multiplier, r += t across all 4 bytes with the
            // incfsz carry idiom. bk is 2 bytes: the low 16 multiplier bits
            // first, then reloaded from b's high half for the second 16
            // iterations (the b param slot is untouched). Store the low 32
            // bits (the i32 result).
            "__mul_u32" => {
                let a = self.slot_addr(name, "a");
                let b = self.slot_addr(name, "b");
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
                    self.emit(format!("    MOVWF 0x{:02X}", t[usize::from(i)])); // t = a (32-bit)
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
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
                self.assert_bank0(&[num, num + 3, den, den + 3, scr, scr + 9], name);
                for i in 0..4 {
                    self.emit(format!("    MOVF 0x{:02X}, W", den + i));
                    self.emit(format!("    MOVWF 0x{:02X}", scr + 4 + i)); // den copy
                }
                self.emit_divmod32(num, scr);
                if name == "__udiv_u32" {
                    self.store_retval(num, 4);
                } else {
                    self.store_retval(scr, 4);
                }
                self.emit("    RETURN".to_string());
            }
            // Signed 32-bit wrappers: abs both operands in place in the
            // param slots (unsigned abs — INT_MIN safe: |INT_MIN| wraps to
            // itself, deterministic), run the unsigned divmod, negate the
            // quotient if the signs differed (bit0 = num<0 XOR den<0) / the
            // remainder if the dividend was negative (bit1).
            "__sdiv_i32" | "__srem_i32" => {
                let num = self.slot_addr(name, "num");
                let den = self.slot_addr(name, "den");
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
                if name == "__sdiv_i32" {
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
            other => panic!("isel: no recipe for runtime routine @{other}"),
        }
    }

    /// The recipe body for the nine variable-count shift routines (i8/i16/
    /// i32). The count arrives UNMASKED (a full i8/i16/i32 — clang emits
    /// it raw); LLVM says counts >= width are poison, so masking to
    /// width-1 keeps the loop bounded (<= 7/15/31 iterations) and yields
    /// the defined-range result: deterministic, documented, never a hang.
    /// The value shifts **in place in the `val` param slot** (the caller's
    /// copy); the masked count runs the loop from `__scr::cnt@0` (the
    /// layout-contract offset). ashr sets C from the sign bit before each
    /// rrf so the sign fills every vacated bit.
    fn emit_shift_body(&mut self, bytes: u16, op: BinOp, scr: u16) {
        let name = self.cur_func;
        let val = self.slot_addr(name, "val");
        let cnt = self.slot_addr(name, "cnt");
        let hi = val + bytes - 1;
        self.assert_bank0(
            &[val, hi, cnt, cnt + bytes - 1, scr, scr + 1],
            name,
        );
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
}

/// Emit one function's body into `g.out`: runtime routines get their recipe
/// body; ordinary functions get the block labels, phi copies, and
/// terminators. Shared by both emission passes — pass A measures the body
/// (every PCLATH restore present) to drive the greedy page assignment, pass
/// B re-emits it with same-page restores skipped.
fn emit_func_body<'m>(g: &mut Gen<'m>, f: &'m ir::Func) {
    // Runtime routines (legalize-injected): the entry block holds only the
    // scratch alloca, so instead of the (empty) block emission the recipe
    // body goes here — the label, the adapted epicurus asm, and the RETURN
    // the injected Func has no `ret` for. A routine with no recipe yet
    // panics loudly rather than emitting an empty label that would silently
    // fall through into the next function.
    if is_routine_name(&f.name) {
        match &f.name[..] {
            "__mul_u8" | "__mul_u16" | "__mul_u32" | "__udiv_u8" | "__urem_u8"
            | "__udiv_u16" | "__urem_u16" | "__udiv_u32" | "__urem_u32"
            | "__sdiv_i8" | "__srem_i8" | "__sdiv_i16" | "__srem_i16"
            | "__sdiv_i32" | "__srem_i32" | "__shl_u8" | "__lshr_u8"
            | "__ashr_i8" | "__shl_u16" | "__lshr_u16" | "__ashr_i16"
            | "__shl_u32" | "__lshr_u32" | "__ashr_i32" => {}
            other => panic!("isel: unknown runtime routine @{other}"),
        }
        g.emit_routine();
        return;
    }
    // Block label scheme: the entry block uses the bare function name
    // (so CALLs and GOTOs resolve to it); every other block is
    // `{func}_L{label}`. The entry block's label is emitted by the block
    // loop below — no standalone function label here, or `main:` /
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
    // phi elimination: for each predecessor block, the copies that must
    // run before it branches into the merge block.
    let mut phi_copies: HashMap<String, Vec<(String, Ty, Val)>> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Phi(p) = i {
                for (val, pred) in &p.incoming {
                    phi_copies
                        .entry(pred.clone())
                        .or_default()
                        .push((p.dst.clone(), p.ty, val.clone()));
                }
            }
        }
    }
    for (i, b) in f.blocks.iter().enumerate() {
        g.emit(format!("{}:", labels[&b.label]));
        if i == 0 && f.isr {
            // The ISR save prologue, right after the vector entry (word 4):
            // W into 0x75, nibble-swapped IN PLACE (SWAPF 0x75, F has no
            // STATUS side effects and no W dependency — the epilogue's
            // swap-back is the flag-safe W restore), STATUS into 0x76
            // nibble-swapped (SWAPF reads STATUS without touching it),
            // PCLATH and FSR into 0x77/0x78, then the preempted main's
            // in-flight return value (0x71-0x74) into 0x79-0x7C — the ISR
            // body's value-returning calls write the retval region, so
            // without this save they would clobber it — then PCLATH = 0 so
            // the ISR body's GOTOs stay in page 0 (the M11 restore literal
            // is PAGE(isr) = 0). The save area is fixed common RAM
            // (0x75-0x7C: W/STATUS/PCLATH/FSR/retval x4 = 8 bytes),
            // disjoint from the scratch byte (0x70) and the retval region
            // (0x71-0x74); 0x7D-0x7F stays free. The retval MOVFs clobber
            // the CURRENT Z, which is harmless — the interrupted STATUS is
            // already safe in 0x76.
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
        if let Some(copies) = phi_copies.get(&b.label) {
            g.emit(format!("    ; phi copies for pred {0}", labels[&b.label]));
            // Emit the predecessor's copies in dependency order so a copy
            // never overwrites a slot a later copy still needs to read.
            // Chains (%a <- %b, %b <- %c) emit c->b then b->a. If the
            // copies form a cycle (%a = phi [%b,P], %b = phi [%a,P] —
            // clang -O1 emits this for loop-carried swaps), no ordering
            // works without a temp register, so panic loudly rather than
            // silently miscompile.
            let pending: Vec<(u16, Option<u16>, Ty, Val)> = copies
                .iter()
                .map(|(dst, ty, val)| {
                    let da = g.slot_addr(g.cur_func, dst);
                    let src = match val {
                        Val::Reg(r) => Some(g.slot_addr(g.cur_func, r)),
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
                    // Blocked while an un-emitted sibling (j != i) writes
                    // this copy's source slot (sibling destination ==
                    // this copy's source).
                    let blocked = match src {
                        Some(s) => {
                            (0..n).any(|j| !emitted[j] && j != i && pending[j].0 == *s)
                        }
                        None => false,
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
        if let Some(t) = terminator {
            if f.isr {
                match t {
                    // The restore epilogue replaces the ISR's `ret`. Order
                    // is load-bearing: the retval region (0x79-0x7C ->
                    // 0x71-0x74) and PCLATH/FSR (MOVF — their Z clobbers
                    // are fine, STATUS is not yet restored), then STATUS
                    // via the nibble swap-back (SWAPF is flag-safe), and W
                    // LAST via its swap-back (also flag-safe — MOVF would
                    // set Z from the moved value after STATUS was already
                    // restored, corrupting the interrupted main's Z).
                    // RETFIE pops the hardware-pushed return.
                    Inst::Ret(None) => {
                        g.emit("    MOVF 0x79, W");
                        g.emit("    MOVWF 0x71");
                        g.emit("    MOVF 0x7A, W");
                        g.emit("    MOVWF 0x72");
                        g.emit("    MOVF 0x7B, W");
                        g.emit("    MOVWF 0x73");
                        g.emit("    MOVF 0x7C, W");
                        g.emit("    MOVWF 0x74");
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
                    _ => g.emit_terminator(t, &labels),
                }
            } else {
                g.emit_terminator(t, &labels);
            }
        }
    }
    g.emit("".to_string());
}

/// The word size of a function's emitted lines: 1 word per instruction line
/// (labels, `.align`/`.table` directives, `equ` lines, comments, and blanks
/// are 0), mirroring the asm crate's pass-1 counting so the page-fit
/// decisions match the addresses the assembler will assign.
fn word_size(lines: &[String]) -> usize {
    lines.iter().filter(|raw| {
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
/// before a function that would cross the current 2048-word page's end — and
/// before ANY function whose start is page-aligned (an overflow continuation
/// or an exact-boundary one) — and return the `(pad, page, next_addr)`. A
/// function larger than one page can never fit (its intra-function GOTOs
/// need a single stable page) and panics loudly; a program past page 3
/// (0x2000 — the device flash) panics loudly too. The `.org` pads with
/// 0x0000 words (the assembler supports it), so the final layout's addresses
/// are exactly what the tracker predicts.
///
/// M11 two-phase emission: pass A measures every function body with all
/// PCLATH restores present and runs this over ALL functions, so a call to a
/// not-yet-emitted target still knows its page; pass B re-emits with
/// same-page restores skipped. The skip only shrinks bodies, and the pads
/// pin the page bases (a function whose start is page-aligned is anchored
/// even when it only continues exactly at the boundary — the strict
/// overflow check alone would leave it unanchored, and the elision would
/// slide it below the boundary into a straddle), so every function stays on
/// the page pass A assigned (elision is page-membership-stable).
fn page_step(addr: usize, size: usize, name: &str) -> (Option<usize>, usize, usize) {
    if size > 0x800 {
        panic!("isel: function @{name} of {size} words exceeds a 2048-word page (0x800)");
    }
    if addr >= 0x2000 {
        panic!(
            "isel: function @{name} would start at 0x{addr:04X}, beyond page 3 (device flash is 8K words)"
        );
    }
    let page_end = (addr & !0x7FF) + 0x800;
    // A function whose start is page-aligned gets an explicit `.org` anchor —
    // even when it merely CONTINUES exactly at the boundary (the previous
    // function's pass-A size hit the boundary precisely, so the strict
    // `addr + size > page_end` overflow check alone would emit no pad). The
    // anchor covers both the overflow case (the pad target IS the
    // page-aligned continuation) and the exact-boundary case, so every
    // page-membership boundary is pinned. Without it, pass B's same-page
    // restore elision shrinks the previous function and slides this one
    // below the boundary into a straddle: its label resolves to the LOWER
    // page while its later words sit in the upper one, so intra-function
    // GOTOs (PAGE(<func>) from the label) misbranch.
    let pad = if addr & 0x7FF == 0 {
        Some(addr)
    } else if addr + size > page_end {
        if page_end >= 0x2000 {
            panic!(
                "isel: function @{name} would start at 0x{page_end:04X}, beyond page 3 (device flash is 8K words)"
            );
        }
        Some(page_end)
    } else {
        None
    };
    let start = pad.unwrap_or(addr);
    (pad, start / 0x800, start + size)
}

/// The const-table section's reader entries and the page PCLATH<4:3> holds
/// after each CALL returns: a reader writes `MOVLW HIGH(<base>); MOVWF
/// PCLATH` before its computed jump, so PCLATH's page bits are the TABLE
/// BASE's page (`<name>` for single/chunk-0 reads, `<name>_1` for chunk-1
/// reads) — not `PAGE(__read_<name>)`, the page the caller set (the entry
/// itself can sit in a different page than its base, e.g. a reader at
/// 0x7FA with a 256-aligned base at 0x800). The caller's restore after the
/// call is needed iff that page differs from its own, so this is the map
/// `emit_pclath_restore` consults. The section's pass-A placement is used:
/// it sits right after the last function, and pass B re-pins the section to
/// this exact start with a leading `.org` whenever the pass-B elision would
/// move a reader base across a page boundary (see the pass-B note in
/// `select`) — so no reader base can drift across a page boundary between
/// the passes — the pages hold in the final text.
fn reader_pages(consts: &[&ir::Global], table_start: usize) -> Vec<(String, usize)> {
    let mut pages = Vec::new();
    let mut addr = table_start;
    for g in consts {
        let size = g.bytes.len();
        if size >= 256 {
            // Reader entry (6 words), `.align 256`, chunk 0 base at the
            // aligned address, chunk 1 base exactly +256 later.
            let aligned = ((addr + 6) + 255) & !255;
            pages.push((format!("__read_{}", g.name), aligned / 0x800));
            pages.push((format!("__read_{}_hi", g.name), (aligned + 256) / 0x800));
            addr = aligned + 256 + (size - 256) + 6;
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
/// allocation — every value's address is read from the map. The icmp scratch
/// byte and the four retval bytes live in fixed common RAM (scratch `0x70`,
/// retval `0x71`-`0x74`): bank-independent, never used by locals (M3), so no
/// BANKSEL is ever needed for them.
///
/// M11: every CALL runs with PCLATH<4:3> = the target's page (set
/// immediately before, restored immediately after — the restore is skipped
/// when the target is in the caller's own page), functions are assigned to
/// 2048-word pages greedily (a function that would cross a page's end gets a
/// `.org <next base>` pad), and the program's highest word address is
/// bounded by the device's 8K-word flash. Emission is two-phase: pass A
/// measures every body (all restores present) and assigns pages for ALL
/// functions so a forward call target's page is known; pass B re-emits with
/// same-page restores skipped (the pads pin the page bases, so the elision
/// never moves a function off its assigned page).
pub fn select(m: &Module, addrs: &HashMap<String, u16>) -> String {
    // The F877A has ONE interrupt vector at word 0x0004 (the hardware pushes
    // the return PC and clears GIE; PCLATH is untouched). The vector IS the
    // ISR entry — no GOTO, since a GOTO's target page would depend on the
    // interrupted PCLATH (unknowable) — so the ISR is emitted FIRST with a
    // `.org 4` pad (words 2-3 after the reset entry), and `__start` moves
    // after it. A second ISR would fight over the single vector: panic
    // loudly.
    let isr_names: Vec<&str> = m
        .funcs
        .iter()
        .filter(|f| f.isr)
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        isr_names.len() <= 1,
        "isel: {} interrupt handlers ({}) — the PIC16F877A has a single vector at word 4",
        isr_names.len(),
        isr_names.join(", ")
    );
    let has_isr = !isr_names.is_empty();
    // The icmp scratch byte and the four retval bytes are fixed common-RAM
    // constants (bank-independent, common RAM 0x70-0x7F is never used by
    // locals, so no collision). The widened i32 region (0x71-0x74) must not
    // overrun common RAM nor overlap the scratch byte — and the ISR save
    // area (0x75-0x78: W, STATUS, PCLATH, FSR) must sit right after the
    // retval region, disjoint from it and from scratch.
    let scratch: u16 = 0x70;
    let retval_lo: u16 = 0x71;
    assert!(
        retval_lo + 4 <= 0x80,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must fit in common RAM",
        retval_lo + 3
    );
    assert!(
        retval_lo + 4 <= 0x75,
        "isel: 4-byte retval region 0x{retval_lo:02X}-0x{:02X} must not overlap the ISR save area 0x75-0x78",
        retval_lo + 3
    );
    assert!(
        0x78 + 1 <= 0x80,
        "isel: ISR save area 0x75-0x78 must fit in common RAM"
    );
    let mut out = vec![
        "; pic8 -- integer spine milestone 2 (isel)".to_string(),
        "    list p=16f877a".to_string(),
        "    radix hex".to_string(),
        "STATUS equ 0x03".to_string(),
        "FSR    equ 0x04".to_string(),
        "INDF   equ 0x00".to_string(),
        "PCL    equ 0x02".to_string(),
        "PCLATH equ 0x0A".to_string(),
        "".to_string(),
        "    org 0x0000".to_string(),
        "    goto __start".to_string(),
        "".to_string(),
    ];
    if !has_isr {
        // No ISR: `__start` sits at the top (word 2) so the reset vector's
        // GOTO (PCLATH = 0 at reset) always reaches it — byte-identical to
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
    // Phase-3 pointers: collect every GEP and resolve its chain eagerly to a
    // folded `(base, k, terms)`, keyed `{func}::{reg}` like every other
    // local. Seeds first: a byval param slot IS the struct copy
    // (Slot(name, false)); an sret param slot holds the target address
    // (Slot(name, true)); an alloca defines its own buffer slot
    // (Slot(name, false)). Gep itself is virtual — it emits nothing.
    let mut geps: HashMap<String, Gep> = HashMap::new();
    let mut resolved: HashMap<String, (Base, u8, Vec<(u8, String)>)> = HashMap::new();
    for f in &m.funcs {
        for p in &f.params {
            if p.byval.is_some() {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), false), 0, Vec::new()),
                );
            } else if p.sret {
                resolved.insert(
                    ssa_key(&f.name, &p.name),
                    (Base::Slot(p.name.clone(), true), 0, Vec::new()),
                );
            }
        }
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Gep(g) => {
                        geps.insert(ssa_key(&f.name, &g.dst), g.clone());
                    }
                    Inst::Alloca(a) => {
                        resolved.insert(
                            ssa_key(&f.name, &a.dst),
                            (Base::Slot(a.dst.clone(), false), 0, Vec::new()),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    // Chain folding (fixpoint scan): a GEP whose base is a `Reg` folds in
    // that reg's own resolved entry — `k` adds, terms concatenate
    // (inner-first: terms_inner + terms_outer) — until the base is a Global
    // or a seeded Slot. A base that is neither a gep nor a seed panics; a
    // pass that makes no progress with unresolved geeps left is a cycle and
    // panics loudly.
    for f in &m.funcs {
        let fname = f.name.clone();
        let mut pending: Vec<(String, Gep)> = geps
            .iter()
            .filter(|(k, _)| k.starts_with(&format!("{fname}::")))
            .map(|(k, g)| (k.clone(), g.clone()))
            .collect();
        let mut progressed = true;
        while progressed {
            progressed = false;
            let mut rest = Vec::new();
            for (key, g) in pending {
                match &g.base {
                    GepBase::Global(name) => {
                        assert!(
                            !resolved.contains_key(&key),
                            "isel: duplicate definition of pointer reg {key}"
                        );
                        resolved.insert(key, (Base::Global(name.clone()), g.k, g.terms.clone()));
                        progressed = true;
                    }
                    GepBase::Reg(r) => {
                        let rkey = ssa_key(&fname, r);
                        if let Some((b, kk, tt)) = resolved.get(&rkey).cloned() {
                            assert!(
                                !resolved.contains_key(&key),
                                "isel: duplicate definition of pointer reg {key}"
                            );
                            let mut terms = tt.clone();
                            terms.extend(g.terms.clone());
                            let k = g.k.checked_add(kk).unwrap_or_else(|| {
                                panic!("isel: gep offset overflow in {key}")
                            });
                            resolved.insert(key, (b, k, terms));
                            progressed = true;
                        } else if geps.contains_key(&rkey) {
                            rest.push((key, g)); // may resolve on a later pass
                        } else {
                            panic!(
                                "isel: no gep for pointer %{r} (chain base missing, key {rkey})"
                            );
                        }
                    }
                }
            }
            pending = rest;
            if !progressed && !pending.is_empty() {
                let names: Vec<&str> = pending.iter().map(|(k, _)| k.as_str()).collect();
                panic!("isel: cyclic gep chain involving {names:?}");
            }
        }
    }
    // Fresh-label counter at module scope: labels are file-scoped in the
    // single `.asm` output, so it must not reset per function.
    // ---- PASS A: emit every function body with every PCLATH restore
    // present, measure word sizes, and run the greedy page assignment over
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
    {
        let mut tmp = 0u32;
        for f in &order {
            let mut g = Gen {
                m,
                addrs,
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
        }
    }
    // Greedy page assignment over every function's pass-A size, in emission
    // order: the `.org` pads and the page each function lands on. The
    // running word address starts at 5 without an ISR — the reset vector (1
    // word: `goto __start`) plus the `__start` body (4 words), with
    // `__start` at the top so the reset vector's GOTO (PCLATH = 0 at reset)
    // always reaches it. With an ISR the vector owns word 4: the ISR is
    // pinned there (no page pad — the vector IS the entry), `__start` moves
    // right after it, and the rest of the program follows. The ISR must fit
    // page 0 (0x004-0x7FF) AND leave `__start` inside page 0 — the reset
    // GOTO runs with PCLATH = 0, so a `__start` at 0x800+ would be
    // unreachable; it panics loudly (ISRs are usually small).
    let mut pages: HashMap<String, usize> = HashMap::new();
    let mut pads: HashMap<String, usize> = HashMap::new();
    let mut addr: usize = if has_isr { 4 } else { 5 };
    for (name, size) in &bodies {
        if has_isr && name == isr_names[0] {
            assert!(
                4 + size <= 0x7FF,
                "isel: isr @{name} of {size} words does not fit page 0 (0x004-0x7FF) with room for the reset __start"
            );
            pages.insert(name.clone(), 0);
            pads.insert(name.clone(), 4);
            addr = 4 + size + 4; // the ISR body + the 4-word __start body
        } else {
            let (pad, page, next) = page_step(addr, *size, name);
            addr = next;
            pages.insert(name.clone(), page);
            if let Some(p) = pad {
                pads.insert(name.clone(), p);
            }
        }
    }
    // Const-table readers: the page PCLATH holds after each `__read_*` CALL
    // (see `reader_pages`). The section sits right after the last function —
    // pass B pins it to this same start, so the pages hold in the final text.
    let mut consts: Vec<&ir::Global> = m.globals.iter().filter(|g| g.is_const).collect();
    consts.sort_by_key(|g| g.name.clone());
    let table_start = addr;
    for (entry, page) in reader_pages(&consts, table_start) {
        pages.insert(entry, page);
    }
    // ---- PASS B: emit the final text with every function's page known.
    // Same-page calls (and same-page const reads) skip the restore pair; the
    // pages are pass A's, and the `.org` pads pin the page bases, so the
    // elision cannot move a function off its assigned page (it only shrinks
    // bodies — page-membership-stable).
    {
        let mut tmp = 0u32;
        let mut addr_b: usize = if has_isr { 4 } else { 5 };
        for (f, (name, _)) in order.iter().zip(&bodies) {
            let mut g = Gen {
                m,
                addrs,
                resolved: &resolved,
                scratch,
                retval_lo,
                cur_func: &f.name,
                tmp: &mut tmp,
                page_of: Some(&pages),
                out: Vec::new(),
            };
            emit_func_body(&mut g, f);
            if let Some(pad) = pads.get(name) {
                out.push(format!("    org 0x{pad:04X}"));
                addr_b = *pad;
            }
            addr_b += word_size(&g.out);
            out.extend(g.out);
            if f.isr {
                // `__start` moves after the ISR (the vector owns word 4):
                // the reset GOTO at word 0 still reaches it — page 0, by
                // the ISR fit check above.
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
        // Pin the const-table section to its pass-A `table_start` whenever
        // the pass-B elision would move a reader base across a page
        // boundary. `reader_pages` maps every reader entry's page from the
        // pass-A position, but pass B emits the tables at the post-elision
        // position (bodies only shrink, so the section shifts earlier) — a
        // chunked table's `.align 256` can then round a base across a page
        // boundary (a base pass A aligned to exactly k*0x800 re-aligns into
        // page k-1 after a 2-word elision), silently invalidating the
        // restore-skip map: a caller that skipped its restore on the mapped
        // page is left with the reader's HIGH(<base>) page — the drifted one
        // — and its next GOTO misbranches. The `.org` re-pins the section so
        // the final addresses are exactly the pass-A ones and the map stays
        // exact — but only when a base's page actually changes (the common
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
                addr = table_start;
            }
        }
    }
    // Const (flash) globals become RETLW tables, emitted after the
    // functions so the CALLs above resolve. Every `__read_<name>` reader
    // sets PCLATH = HIGH(<name>) first — the computed `ADDLW LOW(<name>);
    // MOVWF PCL` jump lands at PCLATH:PCL, so a table in a nonzero 256-byte
    // window needs the window set (the M5 reader left PCLATH stale — the
    // latent window bug). A table of 256+ bytes is emitted as two 256-byte
    // chunks: chunk 0's 256 RETLWs at the base label `<name>` (`.align 256`
    // pads it to a 256-word boundary so LOW(<name>) == 0), then chunk 1's
    // RETLWs at the fresh label `<name>_1` IMMEDIATELY after — `<name>` +
    // 256 in the address space, so LOW(<name>_1) == 0 too and the true
    // bound is 511 bytes (a table of exactly 256 bytes has an empty chunk
    // 1, unreachable since its valid indices are 0..255) — then the
    // `__read_<name>_hi` entry AFTER the
    // table (its computed-goto jumps into the table; the entry instructions
    // are dead after MOVWF PCL). A `.table <name> <size>` directive is
    // emitted immediately before every table's base label; the assembler
    // enforces the window fit loudly (LOW + size <= 0x100 for single-entry
    // tables, LOW == 0 for chunked bases) — a table that crosses its window
    // or a misaligned chunk base would silently misread, so it must fail
    // assembly, not miscompile. Tables beyond 511 bytes (three chunks)
    // panic loudly — out of scope.
    // Label-collision guard: every label a table emits — its base label, its
    // reader entry, and for chunked tables the fresh `{name}_1` chunk label
    // and `__read_{name}_hi` entry — must be unique across all consts. A
    // user `const t_1` (or `const __read_t_hi`) next to a chunked `const t`
    // would emit a duplicate label the assembler's symbol insert silently
    // overwrites (wrong reads, no error) — panic loudly instead.
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
            if g.bytes.len() >= 256 {
                claim(
                    format!("{}_1", g.name),
                    format!("chunk-1 label of const {}", g.name),
                );
                claim(
                    format!("__read_{}_hi", g.name),
                    format!("chunk-1 reader entry of const {}", g.name),
                );
            }
        }
    }
    for g in consts {
        assert!(
            !g.bytes.is_empty(),
            "isel: const @{} has no table bytes",
            g.name
        );
        let size = g.bytes.len();
        assert!(
            size <= 511,
            "isel: const @{} table of {size} bytes exceeds the 511-byte two-chunk bound",
            g.name
        );
        // `MOVLW HIGH` clobbers W, so the incoming index (W = byte index)
        // is stashed in the fixed scratch byte (0x70 — free at a const
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
            // 0), then the `.table` directive, then chunk 0's 256 RETLWs at
            // `name`, chunk 1's RETLWs at `name_1` immediately after
            // (name_1 = name + 256, so LOW(name_1) == 0), and only then the
            // chunk-1 reader entry — AFTER the table. (The entry's computed
            // goto jumps into the table; the entry instructions are dead
            // after MOVWF PCL, so their placement cannot shift the chunks.)
            // A table of exactly 256 bytes gets this branch too (size >=
            // 256): chunk 1 is empty (`name_1:` with no RETLWs, its reader
            // immediately after) and unreachable — every valid index
            // 0..255 selects chunk 0. The old `> 256` cut sent 256-byte
            // tables down the single-entry branch, whose `.table` asserts
            // LOW == 0 for size > 255 — assembly failed unless the layout
            // happened to be aligned; `.align 256` makes it unconditional.
            out.push(format!("__read_{}:", g.name));
            reader(&mut out, &g.name);
            out.push("    .align 256".to_string());
            out.push(format!("    .table {} {size}", g.name));
            out.push(format!("{}:", g.name));
            for b in &g.bytes[..256] {
                out.push(format!("    RETLW 0x{b:02X}"));
            }
            let name_1 = format!("{}_1", g.name);
            out.push(format!("{name_1}:"));
            for b in &g.bytes[256..] {
                out.push(format!("    RETLW 0x{b:02X}"));
            }
            out.push(format!("__read_{}_hi:", g.name));
            reader(&mut out, &name_1);
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
        // stays consistent (tables are unconstrained — their addresses
        // don't affect function placement, which is already decided).
        addr += 6; // reader entry (MOVWF/MOVLW/MOVWF/MOVF/ADDLW/MOVWF PCL)
        if size >= 256 {
            addr = (addr + 255) & !255; // `.align 256`
            addr += 256; // chunk 0 RETLWs
            addr += size - 256; // chunk 1 RETLWs
            addr += 6; // chunk-1 reader entry
        } else {
            addr += size; // single-entry RETLWs
        }
        out.push("".to_string());
    }
    out.push("    end".to_string());
    out.join("\n")
}

/// Parse an alloc-produced address-map text into `HashMap<String, u16>`:
/// `global <name> 0xNN` and `local <func> <name> 0xNN` lines become map
/// entries (locals keyed `{func}::{name}`); `const <name>` lines list flash
/// globals, which have no RAM address, so they are accepted and skipped —
/// isel reads their bytes from the `Module`, never from a RAM slot.
pub fn parse_map(text: &str) -> HashMap<String, u16> {
    let mut addrs = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().expect("map entry");
        match kw {
            "const" => {
                // Flash global: no RAM address; nothing to record.
            }
            "global" => {
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("isel: bad address in map line: {line}"));
                addrs.insert(name, addr);
            }
            "local" => {
                let func = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let name = it
                    .next()
                    .unwrap_or_else(|| panic!("isel: malformed map line: {line}"))
                    .to_string();
                let addr = it
                    .next()
                    .and_then(|h| u16::from_str_radix(h.trim_start_matches("0x"), 16).ok())
                    .unwrap_or_else(|| panic!("isel: bad address in map line: {line}"));
                addrs.insert(format!("{func}::{name}"), addr);
            }
            _ => panic!("isel: unexpected map line: {line}"),
        }
    }
    addrs
}
