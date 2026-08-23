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
