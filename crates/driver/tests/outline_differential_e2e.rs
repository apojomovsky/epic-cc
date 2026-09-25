//! Code factoring (epic-cc#662) against the unfactored program, on the
//! vendored menu demo. The factored listing moves constant tables, so its
//! behaviour is checked through a layout-preserving twin (sites padded
//! with `NOP`s, bodies after the code, long forms only): same selection,
//! same addresses, so the ordered RAM write stream must match exactly.

use pic14_sim::Pic18;
use std::path::PathBuf;
use std::process::Command;

const MENU: &str = "tests/fixtures/vendor/hal-pic18-menu-demo";
const INCLUDES: &[&str] = &[
    "pic18fxx5x-hal/include/epiccc",
    "pic18fxx5x-hal/include",
    "epic-common/include",
    "epic-taskmgr/include",
    "epic-tick/include",
    "epic-lcd/include",
    "epic-serial/include",
    "epic-menu-demo/include",
];
const SOURCES: &[&str] = &[
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c",
    "pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c",
    "pic18fxx5x-hal/src/core/pic18_irq.c",
    "pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c",
    "pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
    "pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c",
    "pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
    "pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c",
    "epic-taskmgr/src/epic_taskmgr.c",
    "epic-tick/src/epic_tick.c",
    "epic-lcd/src/epic_lcd.c",
    "epic-lcd/src/epic_lcd_gpio4.c",
    "epic-serial/src/epic_serial.c",
    "epic-menu-demo/src/menu_demo_core.c",
    "epic-menu-demo/tests/sim_menu_demo.c",
    "config_18F4550.c",
];

fn menu_listing(extra: &[&str]) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(MENU);
    let out = std::env::temp_dir().join(format!(
        "outline-diff-{}-{}.asm",
        std::process::id(),
        extra.len()
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", "18F4550", "--emit", "asm", "-o"])
        .arg(&out);
    for d in ["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"] {
        cmd.args(["-D", d]);
    }
    for i in INCLUDES {
        cmd.arg("-I").arg(root.join(i));
    }
    cmd.args(extra);
    for s in SOURCES {
        cmd.arg(root.join(s));
    }
    let run = cmd.output().expect("spawn epic-cc");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = std::fs::read_to_string(&out).expect("listing");
    let _ = std::fs::remove_file(&out);
    text
}

/// The ordered RAM writes (below PCL and the stack SFRs) of a run, with an
/// interrupt requested after every `irq_every`-th write (0: never).
fn writes(prog: Vec<u16>, steps: usize, irq_every: usize) -> (Vec<(u16, u8)>, bool) {
    let mut sim = Pic18::new(prog);
    let mut out = Vec::new();
    let mut prev = *sim.ram();
    for _ in 0..steps {
        if sim.halted() {
            break;
        }
        sim.step();
        let ram = *sim.ram();
        let mut fire = false;
        for a in 0..0xFF9usize {
            if ram[a] != prev[a] {
                out.push((a as u16, ram[a]));
                fire |= irq_every > 0 && out.len() % irq_every == 0;
            }
        }
        prev = ram;
        if fire {
            // The request's own INTCON flag write is harness input.
            sim.request_interrupt();
            prev = *sim.ram();
        }
    }
    (out, sim.halted())
}

fn assert_prefix_equal(a: &[(u16, u8)], b: &[(u16, u8)], what: &str) {
    let n = a.len().min(b.len());
    assert!(n > 10_000, "{what}: too few writes to mean anything ({n})");
    if let Some(i) = (0..n).find(|&i| a[i] != b[i]) {
        panic!(
            "{what}: write {i} differs: original {:x?}, factored {:x?}",
            &a[i..(i + 4).min(n)],
            &b[i..(i + 4).min(n)]
        );
    }
}

#[test]
fn factored_menu_demo_is_smaller_and_behaves_identically() {
    let plain = menu_listing(&["--no-outline"]);
    assert!(!plain.contains("__pa0:"), "--no-outline must skip the pass");
    let opts = outline::Options::default();
    let factored = outline::factor(&plain, &opts);
    let twin = outline::factor(&plain, &outline::Options { pad: true, ..opts });

    let w_plain = asm::assemble_pic18(&plain);
    let w_factored = asm::assemble_pic18(&factored);
    assert!(
        w_factored.len() + 1000 < w_plain.len(),
        "factoring saved only {} words",
        w_plain.len() as i64 - w_factored.len() as i64
    );

    let w_twin = asm::assemble_pic18(&twin);
    let (a, a_halt) = writes(w_plain.clone(), 400_000, 0);
    let (b, b_halt) = writes(w_twin.clone(), 400_000, 0);
    assert!(a_halt && b_halt, "both runs reach the harness halt");
    assert_eq!(a, b, "to halt, no interrupts");
    for irq in [97, 13] {
        let (a, _) = writes(w_plain.clone(), 300_000, irq);
        let (b, _) = writes(w_twin.clone(), 300_000, irq);
        assert_prefix_equal(&a, &b, &format!("interrupt every {irq} writes"));
    }
}

fn fixture_listing(name: &str, extra: &[&str]) -> String {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let out = std::env::temp_dir().join(format!(
        "outline-{name}-{}-{}.asm",
        std::process::id(),
        extra.len()
    ));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", "18F4550", "--emit", "asm", "-o"])
        .arg(&out);
    cmd.args(extra).arg(src);
    let run = cmd.output().expect("spawn epic-cc");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = std::fs::read_to_string(&out).expect("listing");
    let _ = std::fs::remove_file(&out);
    text
}

/// The default layout itself (near `RCALL` bodies between functions, far
/// `CALL` bodies, tail-merge jumps), unpadded: this program keeps no code
/// address in RAM, so moving code cannot change a single RAM write.
#[test]
fn default_layout_behaves_identically_without_padding() {
    let plain = fixture_listing("outline_mix.c", &["--no-outline"]);
    let factored = fixture_listing("outline_mix.c", &[]);
    for shape in ["RCALL __pa", "    CALL __pa", "GOTO __pa"] {
        assert!(
            factored.contains(shape),
            "fixture no longer exercises {shape}:\n{factored}"
        );
    }
    let w_plain = asm::assemble_pic18(&plain);
    let w_factored = asm::assemble_pic18(&factored);
    assert!(w_factored.len() < w_plain.len());
    let (a, a_halt) = writes(w_plain, 2_000_000, 0);
    let (b, b_halt) = writes(w_factored, 2_000_000, 0);
    assert!(a_halt && b_halt, "both runs reach the halt");
    assert!(
        a.len() > 1000,
        "too few writes to mean anything ({})",
        a.len()
    );
    assert_eq!(a, b);
}

#[test]
fn driver_factors_by_default() {
    let listing = menu_listing(&[]);
    assert!(listing.contains("__pa0:"), "PIC18 builds factor by default");
}
