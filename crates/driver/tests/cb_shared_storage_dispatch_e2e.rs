//! epic-cc#568: a callback stored into a storage global that BOTH contexts
//! dispatch through (the HAL `g_usart->RxCpltCallback(data)` shape, where a
//! param-source struct copy carries the handle into `g_usart`) must appear
//! in every dispatching context's candidate list as the value the store
//! rewrite actually put there. The store rewrite (#137) puts the ISR
//! priority's copy into the shared storage, so MAIN's dispatch sites must
//! list the copy; filtering it out empties the site and isel lowers an
//! empty candidate list to a trap loop.
//!
//! Asserted over the full vendored `hal-pic18-menu-demo` fixture through
//! `merge` + `legalize` in-process, for BOTH contexts' dispatchers, so a
//! single-knob change (classifying without keeping the other context's
//! list sound) fails loudly instead of trading one context's dispatch for
//! the other's.

use std::path::Path;
use std::process::Command;

/// Compile the fixture to LLVM IR with the real binary, then run
/// merge + legalize in-process and return the module.
fn legalized_module() -> ir::Module {
    let menu_demo = "tests/fixtures/vendor/hal-pic18-menu-demo";
    let includes = [
        "pic18fxx5x-hal/include/epiccc",
        "pic18fxx5x-hal/include",
        "epic-common/include",
        "epic-taskmgr/include",
        "epic-tick/include",
        "epic-lcd/include",
        "epic-serial/include",
        "epic-menu-demo/include",
    ];
    let inputs = [
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
    let ll_path = std::env::temp_dir().join(format!("cb_dispatch_568_{}.ll", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_epic-cc"));
    cmd.arg("--target").arg("18F4550").arg("--emit").arg("ll");
    for d in includes {
        cmd.arg("-I").arg(Path::new(menu_demo).join(d));
    }
    for def in ["PIC18F4550", "FOSC_HZ=48000000", "__EPIC_CC__"] {
        cmd.arg("-D").arg(def);
    }
    cmd.arg("-o").arg(&ll_path);
    for f in inputs {
        cmd.arg(Path::new(menu_demo).join(f));
    }
    let out = cmd.output().expect("run epic-cc --emit ll");
    assert!(
        out.status.success(),
        "emit ll failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ll = std::fs::read_to_string(&ll_path).expect("read ll");
    let mut m = irparse::parse_ll_opts(&ll, true);
    m = wholeprog::merge(m);
    legalize::legalize(m)
}

/// Every indirect site in the module, as (function, sorted callees).
fn indirect_sites(m: &ir::Module) -> Vec<(String, Vec<String>)> {
    let defined: std::collections::HashSet<&str> =
        m.funcs.iter().map(|f| f.name.as_str()).collect();
    let mut sites = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let ir::Inst::Call(c) = inst {
                    // An indirect site: numeric func (the SSA register), not
                    // a defined function. Empty candidates are the trap-loop
                    // lowering, so they are surfaced by the caller.
                    if !defined.contains(c.func.as_str()) {
                        let mut cands = c.callees.clone();
                        cands.sort();
                        sites.push((f.name.clone(), cands));
                    }
                }
            }
        }
    }
    sites.sort();
    sites
}

#[test]
fn no_indirect_site_is_left_with_empty_candidates() {
    let m = legalized_module();
    let empty: Vec<_> = indirect_sites(&m)
        .into_iter()
        .filter(|(_, cands)| cands.is_empty())
        .collect();
    assert!(
        empty.is_empty(),
        "indirect sites with empty candidate lists lower to trap loops: {empty:?}"
    );
}

#[test]
fn both_dispatch_contexts_list_the_stored_usart_copies() {
    let m = legalized_module();

    let mut main_sites: Vec<Vec<String>> = Vec::new();
    let mut isr_sites: Vec<Vec<String>> = Vec::new();
    for (f, cands) in indirect_sites(&m) {
        match f.as_str() {
            "epic_dispatch_all_irqs" => main_sites.push(cands),
            "epic_dispatch_all_irqs_isr" => isr_sites.push(cands),
            _ => {}
        }
    }
    assert!(
        !main_sites.is_empty(),
        "main dispatcher has no indirect sites"
    );
    assert!(
        !isr_sites.is_empty(),
        "ISR dispatcher has no indirect sites"
    );

    // The i8-arg USART RX site: the stored value is the priority's copy in
    // both contexts (one shared storage), so both list `epic_serial_on_rx_isr`.
    for (name, sites) in [("main", &main_sites), ("isr", &isr_sites)] {
        assert!(
            sites
                .iter()
                .any(|c| c == &["epic_serial_on_rx_isr".to_string()]),
            "{name} dispatcher's RX site must list epic_serial_on_rx_isr; got {sites:?}"
        );
    }
    // The 0-arg sites: the TxDone and taskmgr callbacks went through the
    // same shared-storage rewrite, plus the main-only originals.
    for (name, sites) in [("main", &main_sites), ("isr", &isr_sites)] {
        assert!(
            sites.iter().any(|c| {
                c.contains(&"epic_serial_on_tx_isr".to_string())
                    && c.contains(&"epic_taskmgr_on_timer0_overflow_isr".to_string())
            }),
            "{name} dispatcher must list the TX and taskmgr copies; got {sites:?}"
        );
    }
    assert!(
        main_sites.iter().any(|c| {
            c.contains(&"epic_tick_on_overflow".to_string()) && c.contains(&"s_tx_cplt".to_string())
        }),
        "main dispatcher keeps the main-only originals; got {main_sites:?}"
    );
}
