//! The ELF+DWARF sidecar encoder: the compile-time artifact gdb loads
//! alongside `epic-cc-gdbserver` (epic-cc#259). It joins the line rows and
//! the typed variable table into one DWARF compile unit with aggregate
//! DIEs (struct/union members, array subranges, enums), every location
//! constant addresses (`DW_OP_addr`, chained with `DW_OP_piece` for a split
//! local) per docs/34 §1, inside an `EM_386` ELF container
//! (docs/34 §3: the session runs as i386, so the sidecar declares the
//! architecture gdb uses).

use alloc::AllocLayout;
use ir::SrcLoc;
use irparse::{DebugVars, DiTypeNode, DiVar};
use iselcore::is_directive;

use gimli::write::{
    Address, AttributeValue, Dwarf, EndianVec, Expression, FileId, LineProgram, LineString,
    Sections, StringTable, Unit, UnitEntryId,
};
use gimli::{constants, Encoding, Format, RunTimeEndian, SectionId};

/// One source row of the phase-1 line table: the C location and the
/// program word address it compiled to.
pub type LineRow = (SrcLoc, u16);

/// Where the join placed a variable.
pub enum VarLoc {
    /// One contiguous object at this address.
    Whole(u16),
    /// A variable SROA split into separately colored pieces, sorted by
    /// offset and non-overlapping. Bytes between pieces have no storage.
    Pieces(Vec<Piece>),
}

/// One byte range of a split variable and the RAM address it landed at.
pub struct Piece {
    pub offset: u32,
    pub size: u32,
    pub addr: u16,
}

/// Join a local's recorded storage against the allocation. A variable
/// with fragment records is described by its pieces alone: the allocator
/// colors each piece's alloca separately, so the first piece's address
/// is not the variable's base. A piece the allocator has no slot for
/// (folded away) leaves a hole; `None` when nothing is mapped.
pub fn local_loc(layout: &AllocLayout, v: &DiVar) -> Option<VarLoc> {
    let func = v.func.as_ref()?;
    let addr_of = |ssa: &String| layout.locals.get(&format!("{func}::{ssa}")).copied();
    if v.pieces.is_empty() {
        return addr_of(v.ssa.as_ref()?).map(VarLoc::Whole);
    }
    let pieces: Vec<Piece> = v
        .pieces
        .iter()
        .filter_map(|p| {
            Some(Piece {
                offset: p.offset,
                size: p.size,
                addr: addr_of(&p.ssa)?,
            })
        })
        .collect();
    (!pieces.is_empty()).then_some(VarLoc::Pieces(pieces))
}

/// One mapped variable of the phase-2 join: a plain C name, its RAM
/// location, and its type-graph root. Names are deliberately flat (no
/// `{func}::` prefix): gdb parses `::` as C++ scope resolution, so a
/// prefixed name is unprintable in a C session. Same-named variables
/// in different functions collide (first in sorted order wins); the
/// phase-2 text artifact keeps the qualified names.
pub struct VarRecord {
    pub name: String,
    pub loc: VarLoc,
    pub ty: u32,
}

/// The variables the join mapped to an address: globals by C name,
/// function-local statics through the `{func}.{name}` symbol key,
/// locals through the `{func}::{ssa}` alloc key.
pub fn joined_vars(layout: &AllocLayout, vars: &DebugVars) -> Vec<VarRecord> {
    let mut out = Vec::new();
    for v in &vars.globals {
        let key = match &v.func {
            Some(func) => format!("{func}.{}", v.name),
            None => v.name.clone(),
        };
        if let Some(&addr) = layout.globals.get(&key) {
            out.push(VarRecord {
                name: v.name.clone(),
                loc: VarLoc::Whole(addr),
                ty: v.ty,
            });
        }
    }
    for v in &vars.locals {
        if let Some(loc) = local_loc(layout, v) {
            out.push(VarRecord {
                name: v.name.clone(),
                loc,
                ty: v.ty,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    // Same-named variables in different functions share one flat C
    // namespace in the DWARF: first in sorted order wins, so the
    // sidecar is deterministic no matter how gdb orders its symtabs.
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(v.name.clone()));
    out
}

/// The DWARF location of a variable: one `DW_OP_addr`, or for a split
/// variable a `DW_OP_piece` chain with an empty piece for every byte that
/// has no storage, so gdb reads those as optimized out instead of
/// reading whatever RAM follows the last piece.
fn location_expr(loc: &VarLoc, size: Option<u32>) -> Expression {
    let mut expr = Expression::raw(Vec::new());
    match loc {
        VarLoc::Whole(addr) => expr.op_addr(Address::Constant(u64::from(*addr))),
        VarLoc::Pieces(pieces) => {
            let mut end = 0;
            for p in pieces {
                if p.offset > end {
                    expr.op_piece(u64::from(p.offset - end));
                }
                expr.op_addr(Address::Constant(u64::from(p.addr)));
                expr.op_piece(u64::from(p.size));
                end = p.offset + p.size;
            }
            if let Some(size) = size.filter(|&size| size > end) {
                expr.op_piece(u64::from(size - end));
            }
        }
    }
    expr
}

/// The line rows the phase-1 table walks out of the final asm text,
/// shared with the textual `--line-table` artifact so the sidecar's
/// `.debug_line` is its DWARF encoding by construction.
pub fn line_rows(asm: &str, locs: &[Option<SrcLoc>]) -> Vec<LineRow> {
    let mut rows = Vec::new();
    let mut org = 0usize;
    let mut li = 0usize;
    for raw in asm.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        let loc = locs.get(li).cloned().flatten();
        li += 1;
        if line.is_empty() || is_directive(line, "list") || is_directive(line, "radix") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("org ") {
            org = usize::from_str_radix(rest.trim().trim_start_matches("0x"), 16).unwrap();
            continue;
        }
        if is_directive(line, "end") {
            break;
        }
        if line.ends_with(':') || line.contains(" equ ") || line.starts_with(".table ") {
            continue;
        }
        if let Some(n) = line.strip_prefix(".align ") {
            let n: usize = n.trim().parse().unwrap();
            org = (org + n - 1) & !(n - 1);
            continue;
        }
        if let Some(loc) = loc {
            rows.push((loc, org as u16));
        }
        org += 1;
    }
    rows
}

/// The entry-level attribute setter; gimli keeps attrs on the entry.
fn set(unit: &mut Unit, id: UnitEntryId, name: gimli::DwAt, value: AttributeValue) {
    unit.get_mut(id).set(name, value);
}

struct TypeEncoder<'a> {
    vars: &'a DebugVars,
    /// Every type is emitted once by metadata id; the map carries the
    /// DIE so cycles cut at the revisit.
    done: std::collections::HashMap<u32, UnitEntryId>,
}

impl<'a> TypeEncoder<'a> {
    fn str_attr(&self, strings: &mut StringTable, s: &str) -> AttributeValue {
        AttributeValue::StringRef(strings.add(s.as_bytes().to_vec()))
    }

    /// Emit `id` (or a resolve-through stand-in) and return its DIE.
    fn emit(
        &mut self,
        strings: &mut StringTable,
        unit: &mut Unit,
        root: UnitEntryId,
        id: u32,
    ) -> UnitEntryId {
        if let Some(&die) = self.done.get(&id) {
            return die;
        }
        match self.vars.types.get(&id) {
            Some(DiTypeNode::Basic {
                name,
                size,
                encoding,
            }) => {
                let die = unit.add(root, constants::DW_TAG_base_type);
                self.done.insert(id, die);
                set(
                    unit,
                    die,
                    constants::DW_AT_name,
                    self.str_attr(strings, name),
                );
                set(
                    unit,
                    die,
                    constants::DW_AT_byte_size,
                    AttributeValue::Udata(u64::from(size.unwrap_or(16) / 8)),
                );
                set(
                    unit,
                    die,
                    constants::DW_AT_encoding,
                    AttributeValue::Data1(ate(encoding)),
                );
                die
            }
            Some(DiTypeNode::Pointer { base, .. }) => {
                let die = unit.add(root, constants::DW_TAG_pointer_type);
                self.done.insert(id, die);
                set(
                    unit,
                    die,
                    constants::DW_AT_byte_size,
                    AttributeValue::Udata(2),
                );
                if let Some(b) = base {
                    let pointee = self.emit(strings, unit, root, *b);
                    set(
                        unit,
                        die,
                        constants::DW_AT_type,
                        AttributeValue::UnitRef(pointee),
                    );
                }
                die
            }
            Some(DiTypeNode::Struct {
                name,
                size,
                members,
            }) => self.emit_composite(
                strings,
                unit,
                root,
                id,
                constants::DW_TAG_structure_type,
                name.clone(),
                *size,
                members,
            ),
            Some(DiTypeNode::Union {
                name,
                size,
                members,
            }) => self.emit_composite(
                strings,
                unit,
                root,
                id,
                constants::DW_TAG_union_type,
                name.clone(),
                *size,
                members,
            ),
            Some(DiTypeNode::Enum { name, size }) => {
                let die = unit.add(root, constants::DW_TAG_enumeration_type);
                self.done.insert(id, die);
                set(
                    unit,
                    die,
                    constants::DW_AT_name,
                    self.str_attr(strings, &name.clone().unwrap_or_default()),
                );
                set(
                    unit,
                    die,
                    constants::DW_AT_byte_size,
                    AttributeValue::Udata(u64::from(size.unwrap_or(16) / 8)),
                );
                die
            }
            Some(DiTypeNode::Array { element, count }) => {
                let die = unit.add(root, constants::DW_TAG_array_type);
                self.done.insert(id, die);
                if let Some(e) = element {
                    let elem = self.emit(strings, unit, root, *e);
                    set(
                        unit,
                        die,
                        constants::DW_AT_type,
                        AttributeValue::UnitRef(elem),
                    );
                }
                let sub = unit.add(die, constants::DW_TAG_subrange_type);
                if let Some(c) = count {
                    set(
                        unit,
                        sub,
                        constants::DW_AT_upper_bound,
                        AttributeValue::Udata(u64::from(c.saturating_sub(1))),
                    );
                }
                die
            }
            // Typedefs and qualifiers resolve to their base (the v1
            // non-goal: no distinct DWARF wrappers).
            Some(DiTypeNode::Typedef { base, .. }) | Some(DiTypeNode::Qualifier { base }) => {
                match base {
                    Some(b) => self.emit(strings, unit, root, *b),
                    None => self.int_fallback(strings, unit, root, id),
                }
            }
            _ => self.int_fallback(strings, unit, root, id),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_composite(
        &mut self,
        strings: &mut StringTable,
        unit: &mut Unit,
        root: UnitEntryId,
        id: u32,
        tag: gimli::DwTag,
        name: Option<String>,
        size: Option<u32>,
        members: &[u32],
    ) -> UnitEntryId {
        let die = unit.add(root, tag);
        // Insert before recursing into members: a member can point back
        // at the composite (linked-list nodes).
        self.done.insert(id, die);
        set(
            unit,
            die,
            constants::DW_AT_name,
            self.str_attr(strings, &name.unwrap_or_default()),
        );
        set(
            unit,
            die,
            constants::DW_AT_byte_size,
            AttributeValue::Udata(u64::from(size.unwrap_or(0) / 8)),
        );
        for &m in members {
            let Some(DiTypeNode::Member { name, offset, ty }) = self.vars.types.get(&m) else {
                continue;
            };
            let mdie = unit.add(die, constants::DW_TAG_member);
            set(
                unit,
                mdie,
                constants::DW_AT_name,
                self.str_attr(strings, &name.clone().unwrap_or_default()),
            );
            if let Some(t) = ty {
                let mdie_ty = self.emit(strings, unit, root, *t);
                set(
                    unit,
                    mdie,
                    constants::DW_AT_type,
                    AttributeValue::UnitRef(mdie_ty),
                );
            }
            if let Some(off) = offset {
                set(
                    unit,
                    mdie,
                    constants::DW_AT_data_member_location,
                    AttributeValue::Udata(u64::from(*off)),
                );
            }
        }
        die
    }

    fn int_fallback(
        &mut self,
        strings: &mut StringTable,
        unit: &mut Unit,
        root: UnitEntryId,
        id: u32,
    ) -> UnitEntryId {
        let die = unit.add(root, constants::DW_TAG_base_type);
        self.done.insert(id, die);
        set(
            unit,
            die,
            constants::DW_AT_name,
            self.str_attr(strings, "int"),
        );
        set(
            unit,
            die,
            constants::DW_AT_byte_size,
            AttributeValue::Udata(2),
        );
        set(
            unit,
            die,
            constants::DW_AT_encoding,
            AttributeValue::Data1(constants::DW_ATE_signed.0),
        );
        die
    }
}

/// Encode the sidecar ELF: `line_rows` become `.debug_line`, the join of
/// `layout` and `vars` becomes variable DIEs with `DW_OP_addr` locations.
pub fn encode(rows: &[LineRow], layout: &AllocLayout, vars: &DebugVars) -> Vec<u8> {
    let encoding = Encoding {
        address_size: 4,
        format: Format::Dwarf32,
        version: 3,
    };
    let mut dwarf = Dwarf::default();
    let (primary_dir, primary_file) = rows
        .first()
        .map(|(l, _)| source_dir_file(&l.file))
        .unwrap_or_else(|| (String::new(), "unknown.c".to_string()));

    let mut line_program = LineProgram::new(
        encoding,
        gimli::LineEncoding::default(),
        LineString::String(primary_dir.clone().into_bytes()),
        LineString::String(primary_file.clone().into_bytes()),
        None,
    );
    let mut files: std::collections::HashMap<String, FileId> = std::collections::HashMap::new();
    let mut dirs: std::collections::HashMap<String, gimli::write::DirectoryId> =
        std::collections::HashMap::new();
    line_program.begin_sequence(None);
    let mut last_addr = 0u64;
    for (loc, addr) in rows {
        let (dir, name) = source_dir_file(&loc.file);
        let key = format!("{dir}/{name}");
        let fid = *files.entry(key).or_insert_with(|| {
            let dir_id = *dirs.entry(dir.clone()).or_insert_with(|| {
                line_program.add_directory(LineString::String(dir.into_bytes()))
            });
            line_program.add_file(LineString::String(name.into_bytes()), dir_id, None)
        });
        let row = line_program.row();
        row.address_offset = u64::from(*addr);
        row.file = fid;
        row.line = u64::from(loc.line);
        row.column = u64::from(loc.col);
        row.is_statement = true;
        line_program.generate_row();
        last_addr = u64::from(*addr);
    }
    line_program.end_sequence(last_addr + 1);

    let mut unit = Unit::new(encoding, line_program);
    let root = unit.root();
    set(
        &mut unit,
        root,
        constants::DW_AT_comp_dir,
        AttributeValue::StringRef(dwarf.strings.add(primary_dir.into_bytes())),
    );
    set(
        &mut unit,
        root,
        constants::DW_AT_producer,
        AttributeValue::StringRef(dwarf.strings.add(b"epic-cc sidecar".to_vec())),
    );
    set(
        &mut unit,
        root,
        constants::DW_AT_name,
        AttributeValue::StringRef(dwarf.strings.add(primary_file.into_bytes())),
    );
    set(
        &mut unit,
        root,
        constants::DW_AT_language,
        AttributeValue::Data2(constants::DW_LANG_C11.0),
    );
    // gdb skips the line table when the CU's pc bounds look like
    // linker-GC'd code (low == high, or low == 0 with no section
    // covering address 0), so bound the CU to its mapped words.
    let first_word = rows.first().map(|(_, a)| u64::from(*a)).unwrap_or(0);
    let last_word = rows.last().map(|(_, a)| u64::from(*a)).unwrap_or(0);
    set(
        &mut unit,
        root,
        constants::DW_AT_low_pc,
        AttributeValue::Address(Address::Constant(first_word)),
    );
    set(
        &mut unit,
        root,
        constants::DW_AT_high_pc,
        AttributeValue::Address(Address::Constant(last_word + 1)),
    );

    let joined = joined_vars(layout, vars);
    let mut types = TypeEncoder {
        vars,
        done: std::collections::HashMap::new(),
    };
    for v in &joined {
        let die = unit.add(root, constants::DW_TAG_variable);
        set(
            &mut unit,
            die,
            constants::DW_AT_name,
            types.str_attr(&mut dwarf.strings, &v.name),
        );
        let ty_die = types.emit(&mut dwarf.strings, &mut unit, root, v.ty);
        set(
            &mut unit,
            die,
            constants::DW_AT_type,
            AttributeValue::UnitRef(ty_die),
        );
        set(
            &mut unit,
            die,
            constants::DW_AT_location,
            AttributeValue::Exprloc(location_expr(&v.loc, vars.type_size(v.ty))),
        );
    }

    dwarf.units.add(unit);

    let mut sections = Sections::new(EndianVec::new(RunTimeEndian::Little));
    dwarf
        .write(&mut sections)
        .expect("sidecar DWARF write cannot fail on an in-memory vec");

    let mut obj = object::write::Object::new(
        object::BinaryFormat::Elf,
        object::Architecture::I386,
        object::Endianness::Little,
    );
    let ids = [
        SectionId::DebugAbbrev,
        SectionId::DebugInfo,
        SectionId::DebugLine,
        SectionId::DebugLineStr,
        SectionId::DebugStr,
        SectionId::DebugStrOffsets,
        SectionId::DebugAddr,
        SectionId::DebugRngLists,
    ];
    // A code section covering the program's word addresses: gdb clips
    // line rows to the objfile's section range, and without any
    // allocated section every row would be discarded.
    let text = obj.add_section(Vec::new(), b".text".to_vec(), object::SectionKind::Text);
    obj.append_section_data(text, &vec![0u8; (last_addr + 1) as usize], 1);
    let text = obj.section_mut(text);
    text.flags = object::SectionFlags::Elf {
        sh_flags: (object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR) as u64,
    };

    for id in ids {
        let Some(bytes) = sections.get(id) else {
            continue;
        };
        let bytes = bytes.slice();
        if bytes.is_empty() {
            continue;
        }
        let sid = obj.add_section(
            Vec::new(),
            id.name().as_bytes().to_vec(),
            object::SectionKind::Debug,
        );
        obj.append_section_data(sid, bytes, 1);
    }
    let mut out = Vec::new();
    obj.emit(&mut out).expect("sidecar ELF write cannot fail");
    out
}

/// Split a source path into an absolute directory plus a bare file
/// name, mirroring what gcc emits.
/// The line rows must land in the same symtab as the variables:
/// gdb concatenates the line directory with the file name for the
/// former but uses the CU name verbatim for the latter, so a relative
/// name on either side splits same-basename symtabs and the rows are
/// never looked up. Absolute on both sides keeps one subfile. Only
/// the sidecar absolutizes; the textual artifacts keep clang's path.
/// The (directory, file) pair for a source path, mirroring what gcc
/// emits: the directory is absolute, the file name is relative to it.
fn source_dir_file(file: &str) -> (String, String) {
    let path = std::path::Path::new(file);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    match (abs.parent(), abs.file_name()) {
        (Some(dir), Some(name)) => (
            dir.to_string_lossy().into_owned(),
            name.to_string_lossy().into_owned(),
        ),
        _ => (String::new(), file.to_string()),
    }
}

/// The DWARF encoding constant for a clang `DW_ATE_*` name.
fn ate(encoding: &str) -> u8 {
    match encoding {
        "DW_ATE_signed" => constants::DW_ATE_signed.0,
        "DW_ATE_unsigned" => constants::DW_ATE_unsigned.0,
        "DW_ATE_signed_char" => constants::DW_ATE_signed_char.0,
        "DW_ATE_unsigned_char" => constants::DW_ATE_unsigned_char.0,
        "DW_ATE_boolean" => constants::DW_ATE_boolean.0,
        "DW_ATE_float" => constants::DW_ATE_float.0,
        _ => constants::DW_ATE_unsigned.0,
    }
}
