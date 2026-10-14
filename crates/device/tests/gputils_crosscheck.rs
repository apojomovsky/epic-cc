//! The device data gate. gputils ships in the dev image, so unlike the ATDF
//! check this cannot be skipped for want of a download.

use device::gputils::{coalesce, ram_from_lkr, LkrRam};
use device::{Core, Device};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Running without the oracle takes two keys, not one: a single variable that
/// turns a green gate into a green non-gate is indistinguishable from a real
/// pass, which is the failure mode this whole file exists to prevent.
const ALLOW: &str = "PIC8_ALLOW_NO_GPUTILS";
const ACCEPT: &str = "PIC8_UNVERIFIED_DEVICE_DATA";
const ACCEPT_VALUE: &str = "i-accept-unverified-device-data";

/// gputils data root, or `None` after both opt-ins and a banner on stderr.
/// A missing tool with one opt-in, or none, fails: a gate that disappears
/// with its tool is not a gate.
fn gputils_share() -> Option<PathBuf> {
    let dir =
        std::env::var("PIC8_GPUTILS_SHARE").unwrap_or_else(|_| "/usr/local/share/gputils".into());
    let path = PathBuf::from(dir);
    // The opt-ins are read only when the tool is genuinely absent, so setting
    // them in a workflow cannot switch off a gate that could have run.
    if path.is_dir() {
        return Some(path);
    }
    assert!(
        std::env::var(ALLOW).is_ok(),
        "gputils data not found at {}. Set PIC8_GPUTILS_SHARE, or {ALLOW}=1 \
         together with {ACCEPT}={ACCEPT_VALUE} to knowingly run without the gate.",
        path.display()
    );
    assert!(
        std::env::var(ACCEPT).as_deref() == Ok(ACCEPT_VALUE),
        "{ALLOW} is set but gputils is missing at {}, so no device number was \
         verified. That is not a pass: also set {ACCEPT}={ACCEPT_VALUE} to \
         record that unverified device data is being accepted deliberately.",
        path.display()
    );
    eprintln!(
        "\n!!! DEVICE DATA UNVERIFIED !!!\n\
         gputils is absent and both opt-ins are set, so no device TOML was\n\
         cross-checked in this run. Nothing here attests flash_words or the\n\
         RAM map. Do not read this suite as evidence the device data is right.\n"
    );
    None
}

/// `p16f877a` -> `16f877a_g.lkr`.
fn lkr_for(share: &Path, name: &str) -> Option<String> {
    let stem = name.strip_prefix('p').unwrap_or(name);
    std::fs::read_to_string(share.join("lkr").join(format!("{stem}_g.lkr"))).ok()
}

/// The `[provenance] tier` of a shipped TOML. Coverage is correlated with it:
/// only the datasheet tier may go uncross-checked.
fn provenance_tier(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("devices")
        .join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.parse::<toml::Value>()
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
        .get("provenance")
        .and_then(|p| p.get("tier"))
        .and_then(|t| t.as_str())
        .unwrap_or_else(|| panic!("{}: no [provenance] tier", path.display()))
        .to_string()
}

fn fmt(rs: &[(u16, u16)]) -> String {
    rs.iter()
        .map(|(a, b)| format!("{a:#06X}-{b:#06X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spans of `a` not covered by `b` (both coalesced, sorted). Span-set
/// subtraction: the residual language R2-R4 classify.
fn subtract(a: &[(u16, u16)], b: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for &(lo, hi) in a {
        let mut cur = u32::from(lo);
        let end = u32::from(hi);
        for &(blo, bhi) in b {
            let (blo, bhi) = (u32::from(blo), u32::from(bhi));
            if bhi < cur || blo > end {
                continue;
            }
            if blo > cur {
                out.push((cur as u16, (blo - 1) as u16));
            }
            cur = cur.max(bhi + 1);
            if cur > end {
                break;
            }
        }
        if cur <= end {
            out.push((cur as u16, hi));
        }
    }
    out
}

fn contains(span: (u16, u16), sub: (u16, u16)) -> bool {
    span.0 <= sub.0 && sub.1 <= span.1
}

/// A `# gputils-divergence:` marker parsed from the device TOML: the span
/// it discloses plus the free-text reason (which cites the .lkr or the
/// datasheet; citation quality is review's job, presence is the gate's).
struct Divergence {
    span: (u16, u16),
    reason: String,
}

/// Machine-greppable disclosures in the raw TOML text. Malformed lines
/// fail loudly rather than reading as absent.
fn parse_markers(name: &str, text: &str) -> (Vec<Divergence>, Vec<String>) {
    let mut markers = Vec::new();
    let mut problems = Vec::new();
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("# gputils-divergence:") else {
            continue;
        };
        let rest = rest.trim();
        let (span_text, reason) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let span = span_text.split_once('-').and_then(|(a, b)| {
            let lo = a
                .trim()
                .strip_prefix("0x")
                .and_then(|h| u16::from_str_radix(h, 16).ok());
            let hi = b
                .trim()
                .strip_prefix("0x")
                .and_then(|h| u16::from_str_radix(h, 16).ok());
            lo.zip(hi)
        });
        match (span, reason.trim().is_empty()) {
            (Some((lo, hi)), false) if lo <= hi => markers.push(Divergence {
                span: (lo, hi),
                reason: reason.trim().into(),
            }),
            _ => problems.push(format!(
                "{name}: malformed `# gputils-divergence:` line: {line}"
            )),
        }
    }
    (markers, problems)
}

/// The raw registry TOML behind a device, the marker source. Synthetic
/// test devices read their base name's real file, which carries no
/// markers today, so unit-test verdicts cannot consume stray disclosures.
fn load_markers(name: &str) -> (Vec<Divergence>, Vec<String>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("devices")
        .join(format!("{name}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_markers(name, &text),
        Err(_) => (Vec::new(), Vec::new()),
    }
}

/// R3: a gputils span the TOML does not claim is accepted when it aliases
/// a claimed span under the part's bank modulus. A span in a bank the
/// TOML already claims is only excused for its SFR prefix (an unclaimed
/// banked extent there is a real underclaim, not an alias); a span in an
/// unclaimed bank may image onto any claimed home bank, and whatever the
/// image leaves uncovered must sit inside that bank page's SFR prefix
/// (0x20 wide on every PIC14, the SFRDataSector shape), never in GPR.
fn alias_redundant(dev: &Device, ours_total: &[(u16, u16)], t: (u16, u16)) -> bool {
    if matches!(dev.core, Core::PicBaseline) {
        // Baseline has no GPR alias banks: only the architectural SFR
        // window (0x00-0x06, mirrored per 0x20 bank page, DS41236E
        // Figure 4-4) is excusable. Anything else unclaimed is real.
        let page = (t.0 / 0x20) * 0x20;
        return contains((page, page + 6), t);
    }
    let mut homes: Vec<u16> = dev.ram_banks.iter().map(|(lo, _)| lo >> 7).collect();
    homes.sort_unstable();
    homes.dedup();
    let len = t.1 - t.0 + 1;
    let tbank = t.0 >> 7;
    let candidates: Vec<u16> = if homes.contains(&tbank) {
        vec![tbank]
    } else {
        homes
    };
    candidates.iter().any(|hb| {
        let ilo = (t.0 & 0x7F) | (hb << 7);
        let image = (ilo, ilo + len - 1);
        let page = ilo & !0x7F;
        let sfr = (page, page + 0x1F);
        subtract(&[image], ours_total)
            .into_iter()
            .all(|gap| contains(sfr, gap))
    })
}

/// Pieces of `span` covered by `cover` (coalesced, sorted).
fn intersect_spans(span: (u16, u16), cover: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for &(lo, hi) in cover {
        if hi < span.0 || lo > span.1 {
            continue;
        }
        out.push((lo.max(span.0), hi.min(span.1)));
    }
    out
}

/// R1-R4 span-set reconciliation (epic-cc#411). R1 compares total coverage
/// as coalesced span sets, kinds ignored: bank-vs-shared partitions with
/// identical extents (p16f819's bank0) and the ADR-034/D-2 splits fold in
/// as the empty-divergence case. Non-empty residuals classify per span:
/// R2 and R4 each need a `# gputils-divergence:` cover; R3 (alias-redundant gputils
/// spans) is computational. A marker pinning a real reserved span is
/// consumed as documentation even with no residual; anything else
/// unused rot-fails the audit.
fn compare_pic14(dev: &Device, lkr: &LkrRam) -> Vec<String> {
    let (markers, mut problems) = load_markers(dev.name);
    problems.extend(compare_pic14_with(dev, lkr, &markers));
    problems
}

fn compare_pic14_with(dev: &Device, lkr: &LkrRam, markers: &[Divergence]) -> Vec<String> {
    let mut problems = Vec::new();
    // Reserved spans: the fixed common window, the D-2 home window, and
    // the W-shadow byte once per region (docs/39 D-2's reconstruction).
    let mut reserved: Vec<(u16, u16)> = Vec::new();
    reserved.extend(dev.common_ram);
    reserved.extend(dev.isr_home_window);
    if let Some(w) = dev.isr_w_shadow {
        reserved.push((w, w));
        for (lo, _) in dev.ram_banks {
            let s = (lo & !0x7F) | (w & 0x7F);
            reserved.push((s, s));
        }
    }
    let reserved = coalesce(&reserved);
    let mut ours = dev.ram_banks.to_vec();
    ours.extend(reserved.iter().copied());
    let ours_total = coalesce(&ours);
    let theirs_total = coalesce(&[lkr.banks.clone(), lkr.shared.clone()].concat());
    let mut consumed = vec![false; markers.len()];
    let cover = |span: (u16, u16), consumed: &mut [bool]| {
        markers.iter().zip(consumed.iter_mut()).any(|(m, used)| {
            if contains(m.span, span) {
                *used = true;
                true
            } else {
                false
            }
        })
    };
    // Each ours-only residual classifies on its own side of the
    // reserved/claimed boundary: R2 for reserved spans the model omits,
    // R4 for claimed spans past the model.
    for q in subtract(&ours_total, &theirs_total) {
        for r in intersect_spans(q, &reserved) {
            if !cover(r, &mut consumed) {
                problems.push(format!(
                    "{}: reserved {} absent from gputils' model with no `# gputils-divergence:` cover (cite {}_g.lkr)",
                    dev.name,
                    fmt(&[r]),
                    dev.name.trim_start_matches('p'),
                ));
            }
        }
        for c in subtract(&[q], &reserved) {
            if !cover(c, &mut consumed) {
                problems.push(format!(
                    "{}: claims {} past gputils' model with no `# gputils-divergence:` cover (cite the datasheet extent)",
                    dev.name,
                    fmt(&[c]),
                ));
            }
        }
    }
    for t in subtract(&theirs_total, &ours_total) {
        if !alias_redundant(dev, &ours_total, t) {
            problems.push(format!(
                "{}: gputils {} has no claimed alias (not a bank-mirror of {})",
                dev.name,
                fmt(&[t]),
                fmt(&ours_total),
            ));
        }
    }
    for (m, used) in markers.iter().zip(consumed.iter()) {
        // A marker pinning a real reserved span is consumed as
        // documentation even when the unions match: the span stays
        // reserved by construction, so the claim cannot rot.
        let pins_reserved = reserved.iter().any(|&r| contains(r, m.span));
        if !used && !pins_reserved {
            problems.push(format!(
                "{}: `# gputils-divergence:` {} ({}) covers no divergence and will rot",
                dev.name,
                fmt(&[m.span]),
                m.reason,
            ));
        }
    }
    problems
}

fn compare(dev: &Device, lkr: &LkrRam) -> Vec<String> {
    let mut problems = Vec::new();
    match dev.core {
        Core::Pic14 | Core::Pic14e | Core::PicBaseline => {
            problems.extend(compare_pic14(dev, lkr));
        }
        Core::Pic18 => {
            // PIC18 has two hardware regions: ACCESSBANK (0x0-0x5F) and the
            // banked GPR banks (0x60-0x7FF). Our `ram_banks` lumps the GPR
            // part of the access window (0x10-0x5F) with the banked banks,
            // so a direct per-bank compare would mismatch. Verify the
            // hardware access window separately and the total allocatable
            // span (GPR plus the fixed retval reservation) against the
            // coalesced hardware total. `fixed_retval` is policy, not
            // hardware, so it is not compared here beyond being part of
            // the total.
            let theirs_access = lkr.access.first().copied();
            if dev.access_bank != theirs_access {
                let show = |r: Option<(u16, u16)>| r.map_or("none".into(), |x| fmt(&[x]));
                problems.push(format!(
                    "{}: access_bank is {} but the first unprotected ACCESSBANK is {}",
                    dev.name,
                    show(dev.access_bank),
                    show(theirs_access)
                ));
            }
            let mut ours = dev.ram_banks.to_vec();
            ours.extend(dev.fixed_retval);
            let mut theirs = lkr.banks.clone();
            theirs.extend(&lkr.access);
            let (ours, theirs) = (coalesce(&ours), coalesce(&theirs));
            if ours != theirs {
                problems.push(format!(
                    "{}: total allocatable RAM disagrees\n  ours   : {}\n  gputils: {}",
                    dev.name,
                    fmt(&ours),
                    fmt(&theirs)
                ));
            }
        }
    }
    problems
}

/// Second oracle for parts gputils 1.5.x does not know (ADR-021): a record
/// transcribed from the DFP `.ini` and attested against the datasheet,
/// vendored under `crates/device/second_oracle/`. It runs only where
/// gputils has no `.lkr` (RAM) or `gpasm -p` rejects the part (flash).
struct SecondOracle {
    flash_words: u32,
    ram_banks: Vec<(u16, u16)>,
    common_ram: Option<(u16, u16)>,
    access_bank: Option<(u16, u16)>,
    isr_w_shadow: Option<u16>,
    isr_home_window: Option<(u16, u16)>,
    pack: String,
    ini: String,
}

fn oracle_field<'a>(v: &'a toml::Value, name: &str, key: &str) -> Result<&'a str, String> {
    match v.get(key).and_then(|x| x.as_str()) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(format!("second oracle {name}: missing or empty `{key}`")),
    }
}

/// One `[lo, hi]` pair; rejects negatives, overflow, and inverted ranges.
fn oracle_pair(v: &toml::Value, name: &str, key: &str) -> Result<(u16, u16), String> {
    let err = || format!("second oracle {name}: `{key}` must hold [lo, hi] with lo <= hi");
    let pair = v.as_array().filter(|xs| xs.len() == 2).ok_or_else(err)?;
    let mut out = [0u16; 2];
    for (i, x) in pair.iter().enumerate() {
        out[i] = u16::try_from(x.as_integer().ok_or_else(err)?).map_err(|_| err())?;
    }
    if out[0] > out[1] {
        return Err(err());
    }
    Ok((out[0], out[1]))
}

fn oracle_opt_pair(v: &toml::Value, name: &str, key: &str) -> Result<Option<(u16, u16)>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(x) => Ok(Some(oracle_pair(x, name, key)?)),
    }
}

fn oracle_opt_u16(v: &toml::Value, name: &str, key: &str) -> Result<Option<u16>, String> {
    let err = || format!("second oracle {name}: `{key}` must hold a u16");
    match v.get(key) {
        None => Ok(None),
        Some(x) => Ok(Some(
            u16::try_from(x.as_integer().ok_or_else(err)?).map_err(|_| err())?,
        )),
    }
}

fn parse_second_oracle(name: &str, text: &str) -> Result<SecondOracle, String> {
    let v: toml::Value = text
        .parse()
        .map_err(|e| format!("second oracle {name}: cannot parse: {e}"))?;
    let got = oracle_field(&v, name, "name")?;
    if got != name {
        return Err(format!("second oracle {name}: record names `{got}`"));
    }
    let flash_words = match v.get("flash_words").and_then(|x| x.as_integer()) {
        Some(n) if n > 0 => u32::try_from(n)
            .map_err(|_| format!("second oracle {name}: flash_words out of range"))?,
        _ => {
            return Err(format!(
                "second oracle {name}: missing or invalid `flash_words`"
            ))
        }
    };
    let banks = v
        .get("ram_banks")
        .and_then(|x| x.as_array())
        .filter(|xs| !xs.is_empty())
        .ok_or_else(|| format!("second oracle {name}: missing or empty `ram_banks`"))?;
    let mut ram_banks = Vec::with_capacity(banks.len());
    for b in banks {
        ram_banks.push(oracle_pair(b, name, "ram_banks")?);
    }
    let common_ram = oracle_opt_pair(&v, name, "common_ram")?;
    let access_bank = oracle_opt_pair(&v, name, "access_bank")?;
    let isr_w_shadow = oracle_opt_u16(&v, name, "isr_w_shadow")?;
    let isr_home_window = oracle_opt_pair(&v, name, "isr_home_window")?;
    let prov = v
        .get("provenance")
        .ok_or_else(|| format!("second oracle {name}: missing [provenance]"))?;
    let pack = oracle_field(prov, name, "pack")?.to_string();
    let ini = oracle_field(prov, name, "ini")?.to_string();
    let sha = oracle_field(prov, name, "ini_sha256")?;
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "second oracle {name}: ini_sha256 must be 64 hex chars"
        ));
    }
    oracle_field(prov, name, "datasheet")?;
    oracle_field(prov, name, "datasheet_section")?;
    // A typo'd key would parse and then never compare, passing both halves
    // while checking nothing: reject anything outside the schema.
    const TOP: &[&str] = &[
        "name",
        "flash_words",
        "ram_banks",
        "common_ram",
        "access_bank",
        "isr_w_shadow",
        "isr_home_window",
        "provenance",
    ];
    if let Some(t) = v.as_table() {
        for k in t.keys() {
            if !TOP.contains(&k.as_str()) {
                return Err(format!("second oracle {name}: unknown key `{k}`"));
            }
        }
    }
    const PROV: &[&str] = &[
        "pack",
        "ini",
        "ini_sha256",
        "datasheet",
        "datasheet_section",
    ];
    if let Some(t) = prov.as_table() {
        for k in t.keys() {
            if !PROV.contains(&k.as_str()) {
                return Err(format!(
                    "second oracle {name}: unknown [provenance] key `{k}`"
                ));
            }
        }
    }
    Ok(SecondOracle {
        flash_words,
        ram_banks,
        common_ram,
        access_bank,
        isr_w_shadow,
        isr_home_window,
        pack,
        ini,
    })
}

fn second_oracle_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("second_oracle")
        .join(format!("{name}.toml"))
}

/// The vendored record, or `None` when the part needs none because gputils
/// covers it. `Some(Err)` is a malformed record, which fails the gate: a
/// broken oracle is not an absent one.
fn load_second_oracle(name: &str) -> Option<Result<SecondOracle, String>> {
    let path = second_oracle_path(name);
    if !path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    Some(parse_second_oracle(name, &text))
}

/// Exact match on the expected TOML values: the record already passed human
/// attestation, so a residual is a defect in one of the two files, never a
/// disclosure. No divergence markers exist on this path by design.
fn compare_second_oracle(dev: &Device, oracle: &SecondOracle) -> Vec<String> {
    let show = |r: Option<(u16, u16)>| r.map_or("none".into(), |x| fmt(&[x]));
    let mut problems = Vec::new();
    if dev.flash_words != oracle.flash_words {
        problems.push(format!(
            "{}: flash_words is {} but the second oracle ({}, {}) states {}",
            dev.name, dev.flash_words, oracle.pack, oracle.ini, oracle.flash_words
        ));
    }
    let (ours, theirs) = (coalesce(dev.ram_banks), coalesce(&oracle.ram_banks));
    if ours != theirs {
        problems.push(format!(
            "{}: ram_banks disagree\n  ours  : {}\n  oracle: {}",
            dev.name,
            fmt(&ours),
            fmt(&theirs)
        ));
    }
    if dev.common_ram != oracle.common_ram {
        problems.push(format!(
            "{}: common_ram is {} but the second oracle states {}",
            dev.name,
            show(dev.common_ram),
            show(oracle.common_ram)
        ));
    }
    if dev.access_bank != oracle.access_bank {
        problems.push(format!(
            "{}: access_bank is {} but the second oracle states {}",
            dev.name,
            show(dev.access_bank),
            show(oracle.access_bank)
        ));
    }
    let show_u16 = |r: Option<u16>| r.map_or("none".into(), |x| format!("0x{x:04X}"));
    if dev.isr_w_shadow != oracle.isr_w_shadow {
        problems.push(format!(
            "{}: isr_w_shadow is {} but the second oracle states {}",
            dev.name,
            show_u16(dev.isr_w_shadow),
            show_u16(oracle.isr_w_shadow)
        ));
    }
    if dev.isr_home_window != oracle.isr_home_window {
        problems.push(format!(
            "{}: isr_home_window is {} but the second oracle states {}",
            dev.name,
            show(dev.isr_home_window),
            show(oracle.isr_home_window)
        ));
    }
    problems
}

/// gpasm names an unknown `-p` part and exits non-zero, which is how the
/// flash gate tells "gputils never heard of this part" from a range verdict.
fn is_unknown_processor(text: &str) -> bool {
    text.contains("Didn't find any processor named")
}

#[test]
fn ram_map_matches_gputils_for_every_device() {
    let Some(share) = gputils_share() else { return };
    let mut checked: Vec<&str> = Vec::new();
    let mut oracle_checked: Vec<&str> = Vec::new();
    let mut uncovered: Vec<&str> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for dev in device::ALL {
        match lkr_for(&share, dev.name) {
            Some(text) => {
                problems.extend(compare(dev, &ram_from_lkr(&text)));
                if second_oracle_path(dev.name).is_file() {
                    problems.push(format!(
                        "{}: gputils knows this part, so its second-oracle record is stale; \
                         remove it",
                        dev.name
                    ));
                }
                checked.push(dev.name);
            }
            // No `.lkr`: gputils never heard of this part, so the vendored
            // second oracle covers it when one exists.
            None => match load_second_oracle(dev.name) {
                Some(Ok(oracle)) => {
                    problems.extend(compare_second_oracle(dev, &oracle));
                    oracle_checked.push(dev.name);
                }
                Some(Err(e)) => problems.push(e),
                None => uncovered.push(dev.name),
            },
        }
    }
    // Coverage is reported by name, never inferred from a count: three covered
    // devices satisfy any `checked > 0` guard while a fourth goes unchecked.
    eprintln!(
        "gputils cross-check: verified {checked:?}, second-oracle {oracle_checked:?}, \
         no .lkr for {uncovered:?}"
    );
    let unattested: Vec<&&str> = uncovered
        .iter()
        .filter(|n| provenance_tier(n) != "datasheet")
        .collect();
    assert!(
        unattested.is_empty(),
        "no gputils .lkr covers {unattested:?}, so nothing verifies their RAM map. \
         Only tier = \"datasheet\" may go uncovered."
    );
    assert!(
        !checked.is_empty() || !oracle_checked.is_empty(),
        "no device was cross-checked"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn widening_access_bank_past_0x5f_fails_the_gate() {
    let lkr = LkrRam {
        banks: vec![(0x60, 0x7FF)],
        shared: Vec::new(),
        access: vec![(0x0, 0x5F)],
    };
    let base = device::PIC18F4550;
    let widened = device::Device {
        access_bank: Some((0x0000, 0x0060)),
        ..base
    };
    let problems = compare(&widened, &lkr);
    assert!(
        !problems.is_empty(),
        "widened access_bank 0x0-0x60 should disagree with gputils 0x0-0x5F"
    );
    assert!(
        problems.iter().any(|p| p.contains("access_bank")),
        "problems should name access_bank: {problems:?}"
    );
}

#[test]
fn single_region_split_passes_when_the_union_matches_gputils() {
    // ADR-034 (docs/39 D-1): PIC16F84's exact shape -- gputils declares the
    // whole GPR as one SHAREBANK (no DATABANK at all), our TOML carves the
    // top 16 bytes into common_ram and leaves the rest as ram_banks.
    let lkr = LkrRam {
        banks: Vec::new(),
        shared: vec![(0x0C, 0x4F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let split = device::Device {
        ram_banks: &[(0x0C, 0x3F)],
        common_ram: Some((0x40, 0x4F)),
        ..base
    };
    let problems = compare(&split, &lkr);
    assert!(
        problems.is_empty(),
        "a split that unions back to gputils' single region should pass: {problems:?}"
    );
}

#[test]
fn single_region_split_that_overclaims_the_region_fails_the_gate() {
    // The same shape, but common_ram reaches past what gputils confirms is
    // bank-independent -- claiming more than the oracle attests must still
    // fail, only claiming less is the allowed policy choice.
    let lkr = LkrRam {
        banks: Vec::new(),
        shared: vec![(0x0C, 0x4F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let overclaimed = device::Device {
        ram_banks: &[(0x0C, 0x3F)],
        common_ram: Some((0x40, 0x50)),
        ..base
    };
    let problems = compare(&overclaimed, &lkr);
    assert!(
        !problems.is_empty(),
        "common_ram reaching past gputils' confirmed region must disagree"
    );
}

/// R1 (epic-cc#411): p16f819's shape, bank0 banked here but SHAREBANK
/// there. Identical extents, different partitions: the union matches, so
/// no allowance is consumed and no disclosure is needed.
#[test]
fn partition_difference_with_identical_extents_passes() {
    let lkr = LkrRam {
        banks: vec![(0xA0, 0xEF), (0x120, 0x16F)],
        shared: vec![(0x20, 0x6F), (0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF), (0x120, 0x16F)],
        common_ram: Some((0x70, 0x7F)),
        ..base
    };
    let problems = compare(&part, &lkr);
    assert!(
        problems.is_empty(),
        "union-equal partitions should pass: {problems:?}"
    );
}

/// R2: a reserved window the oracle omits passes with a disclosure.
#[test]
fn absent_reserved_window_passes_with_disclosure() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F)],
        shared: Vec::new(),
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x20, 0x6F)],
        common_ram: Some((0x70, 0x7F)),
        ..base
    };
    let markers = [Divergence {
        span: (0x70, 0x7F),
        reason: "window omitted from remedy_g.lkr".into(),
    }];
    let problems = compare_pic14_with(&part, &lkr, &markers);
    assert!(
        problems.is_empty(),
        "disclosed omission should pass: {problems:?}"
    );
}

#[test]
fn absent_reserved_window_fails_undisclosed() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F)],
        shared: Vec::new(),
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x20, 0x6F)],
        common_ram: Some((0x70, 0x7F)),
        ..base
    };
    let problems = compare_pic14_with(&part, &lkr, &[]);
    assert!(
        problems.iter().any(|p| p.contains("reserved")),
        "undisclosed omission should name the reserved span: {problems:?}"
    );
}
/// R3: p16f873's shape without the window-mirror excess, so the alias
/// banks alone decide. gpr2 aliases bank0's cells (SFR prefix aside).
#[test]
fn alias_mirror_span_passes_without_disclosure() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F), (0xA0, 0xEF), (0x110, 0x16F)],
        shared: vec![(0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x21, 0x6F), (0xA1, 0xEF)],
        isr_w_shadow: Some(0x20),
        isr_home_window: Some((0x70, 0x7F)),
        ..base
    };
    let problems = compare_pic14_with(&part, &lkr, &[]);
    assert!(problems.is_empty(), "alias banks should pass: {problems:?}");
}

/// R3 negative: genuinely unclaimed banked extents are a defect, whether
/// in a claimed bank (byte 0x20 here) or in an unclaimed one whose home
/// image lands outside claimed GPR (0xA0-0xAF images onto 0x20-0x2F,
/// unclaimed and outside every SFR prefix).
#[test]
fn non_alias_extra_span_fails_the_gate() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F), (0xA0, 0xAF)],
        shared: vec![(0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x21, 0x6F)],
        common_ram: Some((0x70, 0x7F)),
        ..base
    };
    let problems = compare_pic14_with(&part, &lkr, &[]);
    assert!(
        problems.iter().any(|p| p.contains("no claimed alias")),
        "unclaimed GPR extent should fail: {problems:?}"
    );
}

/// R4: the window mirror the TOML claims past the model passes disclosed.
#[test]
fn claimed_excess_passes_with_disclosure() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F), (0xA0, 0xEF)],
        shared: vec![(0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x21, 0x6F), (0xA1, 0xFF)],
        isr_w_shadow: Some(0x20),
        isr_home_window: Some((0x70, 0x7F)),
        ..base
    };
    let markers = [Divergence {
        span: (0xF0, 0xFF),
        reason: "window mirror per 16f873_g.lkr PROTECTED gprnobnk".into(),
    }];
    let problems = compare_pic14_with(&part, &lkr, &markers);
    assert!(
        problems.is_empty(),
        "disclosed excess should pass: {problems:?}"
    );
}

#[test]
fn claimed_excess_fails_undisclosed() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F), (0xA0, 0xEF)],
        shared: vec![(0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x21, 0x6F), (0xA1, 0xFF)],
        isr_w_shadow: Some(0x20),
        isr_home_window: Some((0x70, 0x7F)),
        ..base
    };
    let problems = compare_pic14_with(&part, &lkr, &[]);
    assert!(
        problems.iter().any(|p| p.contains("past gputils' model")),
        "undisclosed excess should fail: {problems:?}"
    );
}

/// The audit: a disclosure covering no divergence rots the file.
#[test]
fn unused_marker_fails_the_gate() {
    let lkr = LkrRam {
        banks: vec![(0xA0, 0xEF)],
        shared: vec![(0x20, 0x6F), (0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF)],
        common_ram: Some((0x70, 0x7F)),
        ..base
    };
    let markers = [Divergence {
        span: (0xF0, 0xFF),
        reason: "stale".into(),
    }];
    let problems = compare_pic14_with(&part, &lkr, &markers);
    assert!(
        problems.iter().any(|p| p.contains("will rot")),
        "unused disclosure should fail: {problems:?}"
    );
}

#[test]
fn malformed_marker_fails_loudly() {
    let (_, problems) = parse_markers(
        "psyn",
        "# gputils-divergence: not-a-span\n# gputils-divergence: 0x0070-0x006F backwards",
    );
    assert_eq!(problems.len(), 2, "both lines should fail: {problems:?}");
    let (markers, problems) = parse_markers(
        "psyn",
        "# gputils-divergence: 0x0070-0x007F window omitted from syn_g.lkr",
    );
    assert!(problems.is_empty() && markers.len() == 1);
}

/// R3 scoping (review): the 0x00-0x1F SFR-prefix excuse is PIC14
/// geometry. The same span pair passes on pic14 but fails on baseline,
/// where 0x07-0x0F is real GPR behind a 0x00-0x06 SFR window.
#[test]
fn alias_sfr_geometry_is_pic14_only() {
    let lkr = LkrRam {
        banks: vec![(0x00, 0x1F)],
        shared: vec![],
        access: vec![],
    };
    let pic14 = device::Device {
        ram_banks: &[(0x10, 0x1F)],
        common_ram: None,
        ..device::PIC16F887
    };
    assert!(compare_pic14_with(&pic14, &lkr, &[]).is_empty());
    let baseline = device::Device {
        core: Core::PicBaseline,
        ram_banks: &[(0x10, 0x1F)],
        common_ram: None,
        ..device::PIC16F887
    };
    assert!(compare_pic14_with(&baseline, &lkr, &[])
        .iter()
        .any(|p| p.contains("no claimed alias")));
}

/// Reserved-pin consumption (epic-cc#406 review): the truncated D-2
/// shape unions clean, so its window marker covers no residual; the
/// marker is consumed for pinning the real reserved span instead of
/// rot-failing.
#[test]
fn window_marker_pins_reserved_span_without_residual() {
    let lkr = LkrRam {
        banks: vec![(0x20, 0x6F), (0xA0, 0xEF), (0x110, 0x16F), (0x190, 0x1EF)],
        shared: vec![(0x70, 0x7F)],
        access: Vec::new(),
    };
    let base = device::PIC16F887;
    let part = device::Device {
        ram_banks: &[(0x21, 0x6F), (0xA1, 0xEF)],
        isr_w_shadow: Some(0x20),
        isr_home_window: Some((0x70, 0x7F)),
        ..base
    };
    let markers = [Divergence {
        span: (0x70, 0x7F),
        reason: "reserved isr_home_window, real RAM".into(),
    }];
    let problems = compare_pic14_with(&part, &lkr, &markers);
    assert!(
        problems.is_empty(),
        "pinned window marker should pass: {problems:?}"
    );
}

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// One `org` probe: whether gpasm ran at all, and its combined output.
/// gpasm exits 0 on a range warning, so the caller matches on the text; a
/// non-zero exit means gpasm never assessed the address.
struct Probe {
    ran: bool,
    text: String,
}

fn probe_org(dev_name: &str, addr: u32, tag: &str) -> Probe {
    let dir = std::env::temp_dir();
    let asm = dir.join(format!("probe_{dev_name}_{tag}.asm"));
    let hex = dir.join(format!("probe_{dev_name}_{tag}.hex"));
    std::fs::write(&asm, format!("    org 0x{addr:X}\n    nop\n    end\n")).unwrap();
    let out = Command::new(gpasm())
        .args([
            "-p",
            dev_name,
            asm.to_str().unwrap(),
            "-o",
            hex.to_str().unwrap(),
        ])
        .output()
        .expect("gpasm must be runnable; set PIC8_GPASM");
    Probe {
        ran: out.status.success(),
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

const OVERFLOW: &str = "Address exceeds maximum range";

#[test]
fn flash_words_matches_gputils_for_every_device() {
    let Some(share) = gputils_share() else {
        return;
    };
    let mut oracle_checked: Vec<&str> = Vec::new();
    for dev in device::ALL {
        // org counts words on PIC14 and bytes on PIC18, so the last valid
        // address and the first bad one differ per core.
        let (last, past) = match dev.core {
            Core::Pic14 | Core::Pic14e | Core::PicBaseline => {
                (dev.flash_words - 1, dev.flash_words)
            }
            Core::Pic18 => (dev.flash_words * 2 - 2, dev.flash_words * 2),
        };

        let inside = probe_org(dev.name, last, "last");
        // gpasm never heard of this part: neither probe can run, so the
        // vendored second oracle states flash_words instead. A part with a
        // `.lkr` but no `gpasm` entry is a broken install, not an oracle
        // case: fail on the mix rather than split the halves across oracles.
        if !inside.ran && is_unknown_processor(&inside.text) {
            if lkr_for(&share, dev.name).is_some() {
                panic!(
                    "{}: mixed gputils state: a .lkr exists but gpasm rejects -p {}; \
                     fix the install instead of adding a record (ADR-021)",
                    dev.name, dev.name
                );
            }
            match load_second_oracle(dev.name) {
                Some(Ok(oracle)) => {
                    assert_eq!(
                        dev.flash_words, oracle.flash_words,
                        "{}: flash_words is {} but the second oracle ({}, {}) states {}",
                        dev.name, dev.flash_words, oracle.pack, oracle.ini, oracle.flash_words
                    );
                    oracle_checked.push(dev.name);
                    continue;
                }
                Some(Err(e)) => panic!("{}: malformed second-oracle record: {e}", dev.name),
                None => panic!(
                    "{}: gpasm does not know this part and no second-oracle record covers it; \
                     add crates/device/second_oracle/{}.toml (ADR-021)",
                    dev.name, dev.name
                ),
            }
        }
        assert!(
            inside.ran,
            "{}: gpasm could not run for this device, so 0x{last:X} was never \
             assessed (an unknown -p exits non-zero listing its processors):\n{}",
            dev.name, inside.text
        );
        assert!(
            !inside.text.contains(OVERFLOW),
            "{}: gpasm rejects 0x{last:X}, which flash_words = {} claims exists:\n{}",
            dev.name,
            dev.flash_words,
            inside.text
        );

        let outside = probe_org(dev.name, past, "past");
        assert!(
            outside.ran || outside.text.contains(OVERFLOW),
            "{}: gpasm could not run for this device, so 0x{past:X} was never \
             assessed; this is not evidence about flash_words:\n{}",
            dev.name,
            outside.text
        );
        assert!(
            outside.text.contains(OVERFLOW),
            "{}: gpasm accepts 0x{past:X}, past the {} words flash_words claims:\n{}",
            dev.name,
            dev.flash_words,
            outside.text
        );
    }
    eprintln!("gputils flash cross-check: second-oracle {oracle_checked:?}");
}

/// The pack-derived SFR table against gputils' own headers: every register
/// gputils names must be in the table, by name or pack alias, at the same
/// address, and every single-bit field both name must sit at the same bit.
/// Bits only one side names are MPASM spellings (`R_NOT_W`), not errors.
#[test]
fn sfr_tables_match_gputils_headers() {
    let Some(share) = gputils_share() else {
        return;
    };
    for dev in device::ALL.iter().filter(|d| !d.sfrs.is_empty()) {
        let stem = dev.name.strip_prefix('p').unwrap_or(dev.name);
        let path = share.join("header").join(format!("p{stem}.inc"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: cannot read {}: {e}", dev.name, path.display()));
        let inc = device::gputils::names_from_inc(&text);
        let find = |name: &str| {
            dev.sfrs
                .iter()
                .find(|s| s.name == name || s.aliases.contains(&name))
        };

        for (name, addr) in &inc.registers {
            let s = find(name).unwrap_or_else(|| {
                panic!(
                    "{}: gputils register {name} is not in the sfrs table",
                    dev.name
                )
            });
            assert_eq!(
                s.addr, *addr,
                "{}: {name} is at 0x{:04X} in the TOML, 0x{addr:04X} in gputils",
                dev.name, s.addr
            );
        }

        for (reg, bit, pos) in &inc.bits {
            let Some(s) = find(reg) else { continue };
            let field = s
                .fields
                .iter()
                .find(|f| f.name == bit && f.mask.count_ones() == 1);
            if let Some(f) = field {
                assert_eq!(
                    f.shift, *pos,
                    "{}: {reg}.{bit} is bit {} in the TOML, bit {pos} in gputils",
                    dev.name, f.shift
                );
            }
        }
    }
}

/// Every vendored record names a shipped device and parses: a record for a
/// removed part, or one that stopped parsing, must fail here rather than
/// sit unverified until its part goes through the gate.
#[test]
fn second_oracle_records_match_known_devices() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("second_oracle");
    let mut problems = Vec::new();
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if !device::ALL.iter().any(|d| d.name == stem) {
            problems.push(format!("second oracle {stem}: names no known device"));
            continue;
        }
        if let Some(Err(e)) = load_second_oracle(&stem) {
            problems.push(e);
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn oracle_fixture(name: &str) -> String {
    format!(
        "name = \"{name}\"\n\
         flash_words = 8192\n\
         ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]\n\
         common_ram = [0x0070, 0x007F]\n\
         [provenance]\n\
         pack = \"Microchip.PIC16F1xxxx_DFP.1.29.444\"\n\
         ini = \"xc8/pic/dat/ini/16f13145.ini\"\n\
         ini_sha256 = \"af0c878c8b154a15a6fda72b216cb9642ddab13b4f36aeeb2f5340e6415de564\"\n\
         datasheet = \"DS40002519D\"\n\
         datasheet_section = \"9.1 Table 9-1 (flash), 9.2 Figure 9-4 (RAM)\"\n"
    )
}

fn oracle_device() -> (Device, SecondOracle) {
    let base = device::PIC16F887;
    let dev = Device {
        flash_words: 8192,
        ram_banks: &[(0x20, 0x6F), (0xA0, 0xEF)],
        common_ram: Some((0x70, 0x7F)),
        access_bank: None,
        ..base
    };
    let oracle = parse_second_oracle("p9z9", &oracle_fixture("p9z9")).unwrap();
    (dev, oracle)
}

#[test]
fn second_oracle_parse_accepts_a_full_record() {
    let o = parse_second_oracle("p9z9", &oracle_fixture("p9z9")).unwrap();
    assert_eq!(o.flash_words, 8192);
    assert_eq!(o.ram_banks, vec![(0x20, 0x6F), (0xA0, 0xEF)]);
    assert_eq!(o.common_ram, Some((0x70, 0x7F)));
    assert_eq!(o.access_bank, None);
}

#[test]
fn second_oracle_parse_rejects_a_nameless_or_shaless_record() {
    assert!(parse_second_oracle("p9z9", "flash_words = 1\nram_banks = [[0, 1]]").is_err());
    let no_sha = oracle_fixture("p9z9").replace("ini_sha256", "nope_sha");
    assert!(parse_second_oracle("p9z9", &no_sha).is_err());
    let short_sha = oracle_fixture("p9z9").replace("af0c878c", "zz");
    assert!(parse_second_oracle("p9z9", &short_sha).is_err());
    let renamed = oracle_fixture("p9z8");
    assert!(parse_second_oracle("p9z9", &renamed).is_err());
    let typo = oracle_fixture("p9z9").replace("common_ram", "common_rma");
    assert!(parse_second_oracle("p9z9", &typo).is_err());
}

#[test]
fn second_oracle_compare_passes_on_identical_values() {
    let (dev, oracle) = oracle_device();
    let problems = compare_second_oracle(&dev, &oracle);
    assert!(
        problems.is_empty(),
        "identical values should pass: {problems:?}"
    );
}

#[test]
fn second_oracle_compare_names_each_mismatch() {
    let (base_dev, base_oracle) = oracle_device();
    let small_flash = Device {
        flash_words: 4096,
        ..base_dev
    };
    assert!(
        compare_second_oracle(&small_flash, &base_oracle)
            .iter()
            .any(|p| p.contains("flash_words")),
        "flash mismatch should name flash_words"
    );
    let mut short_oracle = parse_second_oracle("p9z9", &oracle_fixture("p9z9")).unwrap();
    short_oracle.ram_banks.pop();
    assert!(
        compare_second_oracle(&base_dev, &short_oracle)
            .iter()
            .any(|p| p.contains("ram_banks")),
        "bank mismatch should name ram_banks"
    );
    let mut moved_oracle = parse_second_oracle("p9z9", &oracle_fixture("p9z9")).unwrap();
    moved_oracle.common_ram = Some((0x60, 0x6F));
    assert!(
        compare_second_oracle(&base_dev, &moved_oracle)
            .iter()
            .any(|p| p.contains("common_ram")),
        "window mismatch should name common_ram"
    );
    let drifted_dev = Device {
        isr_home_window: Some((0x60, 0x6F)),
        ..base_dev
    };
    assert!(
        compare_second_oracle(&drifted_dev, &base_oracle)
            .iter()
            .any(|p| p.contains("isr_home_window")),
        "carve mismatch should name isr_home_window"
    );
}

#[test]
fn unknown_processor_matches_gpasm_verdict() {
    assert!(is_unknown_processor(
        "Didn't find any processor named: p16f13145\nHere are the supported processors:"
    ));
    assert!(!is_unknown_processor("Address exceeds maximum range"));
    assert!(!is_unknown_processor(""));
}
