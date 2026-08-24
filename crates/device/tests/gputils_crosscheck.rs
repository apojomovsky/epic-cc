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

/// Reports every bank whose bounds differ, plus any bank one side does not
/// have at all, so a diff names the range rather than a bare boolean.
fn bank_diff(ours: &[(u16, u16)], theirs: &[(u16, u16)]) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..ours.len().max(theirs.len()) {
        match (ours.get(i), theirs.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (a, b) => out.push(format!(
                "  bank {i}: ours {} vs gputils {}",
                a.map_or("absent".into(), |r| fmt(&[*r])),
                b.map_or("absent".into(), |r| fmt(&[*r]))
            )),
        }
    }
    out
}

fn compare(dev: &Device, lkr: &LkrRam) -> Vec<String> {
    let mut problems = Vec::new();
    match dev.core {
        Core::Pic14 | Core::Pic14e => {
            // Banked GPR and the common window are compared apart. `isel`
            // derives `fsr_window` from where that boundary sits, so a merged
            // total would accept a bank that grew into the common range.
            let diff = bank_diff(&coalesce(dev.ram_banks), &coalesce(&lkr.banks));
            if !diff.is_empty() {
                problems.push(format!(
                    "{}: ram_banks disagree\n{}",
                    dev.name,
                    diff.join("\n")
                ));
            }
            let theirs = lkr.shared.first().copied();
            if dev.common_ram != theirs {
                let show = |r: Option<(u16, u16)>| r.map_or("none".into(), |x| fmt(&[x]));
                problems.push(format!(
                    "{}: common_ram is {} but the first unprotected SHAREBANK is {}",
                    dev.name,
                    show(dev.common_ram),
                    show(theirs)
                ));
            }
        }
        Core::Pic18 => {
            // Named exception, unresolved: `p18f4550` ships common_ram
            // [0x0,0xF] while the .lkr access RAM is [0x0,0x5F]. On PIC18 our
            // common_ram is isel-pic18's fixed retval reservation carved out
            // of access RAM (see isel-pic18::select), a compiler choice the
            // .lkr cannot attest, so only the total span is comparable.
            let mut ours = dev.ram_banks.to_vec();
            ours.extend(dev.common_ram);
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

#[test]
fn ram_map_matches_gputils_for_every_device() {
    let Some(share) = gputils_share() else { return };
    let mut checked: Vec<&str> = Vec::new();
    let mut uncovered: Vec<&str> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for dev in device::ALL {
        match lkr_for(&share, dev.name) {
            Some(text) => {
                problems.extend(compare(dev, &ram_from_lkr(&text)));
                checked.push(dev.name);
            }
            None => uncovered.push(dev.name),
        }
    }
    // Coverage is reported by name, never inferred from a count: three covered
    // devices satisfy any `checked > 0` guard while a fourth goes unchecked.
    eprintln!("gputils cross-check: verified {checked:?}, no .lkr for {uncovered:?}");
    let unattested: Vec<&&str> = uncovered
        .iter()
        .filter(|n| provenance_tier(n) != "datasheet")
        .collect();
    assert!(
        unattested.is_empty(),
        "no gputils .lkr covers {unattested:?}, so nothing verifies their RAM map. \
         Only tier = \"datasheet\" may go uncovered."
    );
    assert!(!checked.is_empty(), "no device was cross-checked");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
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
    let Some(_share) = gputils_share() else {
        return;
    };
    for dev in device::ALL {
        // org counts words on PIC14 and bytes on PIC18, so the last valid
        // address and the first bad one differ per core.
        let (last, past) = match dev.core {
            Core::Pic14 | Core::Pic14e => (dev.flash_words - 1, dev.flash_words),
            Core::Pic18 => (dev.flash_words * 2 - 2, dev.flash_words * 2),
        };

        let inside = probe_org(dev.name, last, "last");
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
}
