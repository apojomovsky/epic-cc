//! epic-cc#918: the pristine menu demo reaches its first Timer2 tick.
//!
//! The tick callback rides a caller-local handle into an uninlined
//! `EPIC_TIMER2_Init` memcpy, an edge legalize used to miss: the ISR site
//! then listed every 0-arg callback except the tick, and the first tick
//! trapped with `g_tick_ms` at 0. This test runs the real menu listing
//! with the Timer2 model and asserts the counter advances.

use pic14_sim::Pic18;
use std::path::PathBuf;
use std::process::Command;

const BASE: &str = "tests/fixtures/vendor/hal-pic18-base";
const MENU: &str = "tests/fixtures/vendor/hal-pic18-menu-demo";
const INCLUDES: &[(&str, &str)] = &[
    (BASE, "pic18fxx5x-hal/include/epiccc"),
    (BASE, "pic18fxx5x-hal/include"),
    (BASE, "epic-common/include"),
    (BASE, "epic-taskmgr/include"),
    (BASE, "epic-tick/include"),
    (MENU, "epic-lcd/include"),
    (BASE, "epic-serial/include"),
    (MENU, "epic-menu-demo/include"),
];
const SOURCES: &[(&str, &str)] = &[
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c"),
    (BASE, "pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c"),
    (BASE, "pic18fxx5x-hal/src/core/pic18_irq.c"),
    (BASE, "pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c"),
    (
        BASE,
        "pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c",
    ),
    (BASE, "pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c"),
    (
        BASE,
        "pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c",
    ),
    (MENU, "pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c"),
    (BASE, "epic-taskmgr/src/epic_taskmgr.c"),
    (BASE, "epic-tick/src/epic_tick.c"),
    (MENU, "epic-lcd/src/epic_lcd.c"),
    (MENU, "epic-lcd/src/epic_lcd_gpio4.c"),
    (BASE, "epic-serial/src/epic_serial.c"),
    (MENU, "epic-menu-demo/src/menu_demo_core.c"),
    (MENU, "epic-menu-demo/tests/sim_menu_demo.c"),
    (MENU, "config_18F4550.c"),
];

/// Compile the pristine (`--no-outline`) menu demo to HEX plus the address
/// map, via the real driver binary.
fn compile_menu() -> (String, String) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir();
    let tag = format!(
        "menu-tick-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let hex_path = dir.join(format!("{tag}.hex"));
    let map_path = dir.join(format!("{tag}.map"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.args(["--target", "18F4550", "--emit", "hex", "--no-outline"]);
    cmd.arg("-o").arg(&hex_path).arg("--map").arg(&map_path);
    for d in ["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"] {
        cmd.args(["-D", d]);
    }
    for (r, i) in INCLUDES {
        cmd.arg("-I").arg(manifest.join(r).join(i));
    }
    for (r, s) in SOURCES {
        cmd.arg(manifest.join(r).join(s));
    }
    let run = cmd.output().expect("spawn epic-cc");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let hex = std::fs::read_to_string(&hex_path).expect("read hex");
    let map = std::fs::read_to_string(&map_path).expect("read map");
    let _ = std::fs::remove_file(&hex_path);
    let _ = std::fs::remove_file(&map_path);
    (hex, map)
}
#[test]
fn menu_demo_reaches_first_timer2_tick() {
    let (hex, map) = compile_menu();
    let prefix = "global g_tick_ms 0x";
    let line = map
        .lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no map entry for g_tick_ms in:\n{map}"));
    let tick_addr =
        usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex");
    let mut sim = Pic18::new(pic14_sim::parse_hex_pic18(&hex));
    sim.set_timer2_enabled(true);
    let mut steps = 0usize;
    loop {
        let ram = sim.ram();
        let tick = u32::from_le_bytes([
            ram[tick_addr],
            ram[tick_addr + 1],
            ram[tick_addr + 2],
            ram[tick_addr + 3],
        ]);
        if tick != 0 {
            break;
        }
        assert!(
            steps < 1_000_000,
            "menu demo never reached the first Timer2 tick (pc={:#06x}); \
             the ISR dispatch trapped before g_tick_ms advanced",
            sim.pc()
        );
        assert!(!sim.halted(), "menu demo halted before the first tick");
        sim.step();
        steps += 1;
    }
}
