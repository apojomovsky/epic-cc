//! The device data gate. gputils ships in the dev image, so unlike the ATDF
//! check this cannot be skipped for want of a download.

use device::gputils::{coalesce, gpr_ranges_from_lkr};
use std::path::PathBuf;

/// gputils data root. A missing tool fails rather than skips: a gate that
/// disappears with its tool is not a gate.
fn gputils_share() -> Option<PathBuf> {
    if std::env::var("PIC8_ALLOW_NO_GPUTILS").is_ok() {
        return None;
    }
    let dir =
        std::env::var("PIC8_GPUTILS_SHARE").unwrap_or_else(|_| "/usr/local/share/gputils".into());
    let path = PathBuf::from(dir);
    assert!(
        path.is_dir(),
        "gputils data not found at {}. Set PIC8_GPUTILS_SHARE, or \
         PIC8_ALLOW_NO_GPUTILS=1 to knowingly run without the gate.",
        path.display()
    );
    Some(path)
}

/// `p16f877a` -> `16f877a_g.lkr`.
fn lkr_for(share: &PathBuf, name: &str) -> Option<String> {
    let stem = name.strip_prefix('p').unwrap_or(name);
    let path = share.join("lkr").join(format!("{stem}_g.lkr"));
    std::fs::read_to_string(path).ok()
}

#[test]
fn ram_map_matches_gputils_for_every_device() {
    let Some(share) = gputils_share() else { return };
    let mut checked = 0;
    for dev in device::ALL {
        let Some(lkr) = lkr_for(&share, dev.name) else {
            // No .lkr means this part is not covered; provenance must then be
            // the datasheet tier, which Task 1's rule already enforces.
            continue;
        };
        let mut ours: Vec<(u16, u16)> = dev.ram_banks.to_vec();
        if let Some(cr) = dev.common_ram {
            ours.push(cr);
        }
        let ours = coalesce(&ours);
        let theirs = coalesce(&gpr_ranges_from_lkr(&lkr));
        let fmt = |rs: &[(u16, u16)]| {
            rs.iter()
                .map(|(a, b)| format!("{a:#06X}-{b:#06X}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            ours,
            theirs,
            "{}: RAM map disagrees with gputils\n  ours   : {}\n  gputils: {}",
            dev.name,
            fmt(&ours),
            fmt(&theirs)
        );
        checked += 1;
    }
    assert!(checked > 0, "no device was cross-checked");
}

use device::Core;
use std::process::Command;

fn gpasm() -> String {
    std::env::var("PIC8_GPASM").unwrap_or_else(|_| "gpasm".into())
}

/// Assembles `org <addr>` for `dev` and returns gpasm's combined output.
/// gpasm exits 0 on a range warning, so the caller matches on the text.
fn probe_org(dev_name: &str, addr: u32, tag: &str) -> String {
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
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
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
            !inside.contains(OVERFLOW),
            "{}: gpasm rejects 0x{last:X}, which flash_words = {} claims exists:\n{inside}",
            dev.name,
            dev.flash_words
        );

        let outside = probe_org(dev.name, past, "past");
        assert!(
            outside.contains(OVERFLOW),
            "{}: gpasm accepts 0x{past:X}, past the {} words flash_words claims:\n{outside}",
            dev.name,
            dev.flash_words
        );
    }
}
