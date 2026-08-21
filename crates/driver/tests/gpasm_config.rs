use std::process::Command;

fn gpasm_hex(asm_path: &str) -> String {
    let out_path = format!("{asm_path}.hex");
    let status = Command::new("gpasm")
        .args(["-o", &out_path, asm_path])
        .status()
        .expect("run gpasm");
    assert!(status.success(), "gpasm failed on {asm_path}");
    std::fs::read_to_string(&out_path).expect("read gpasm hex")
}

/// Pull the data bytes out of the one :02 record at `want_addr` (an Intel
/// HEX line like `:02400E00713F00`: count, address, type, data..., sum).
fn bytes_at(hex: &str, want_addr: u32) -> Vec<u8> {
    for line in hex.lines() {
        let rec = line.trim_start_matches(':');
        if rec.len() < 8 {
            continue;
        }
        let count = u8::from_str_radix(&rec[0..2], 16).unwrap();
        let addr = u16::from_str_radix(&rec[2..6], 16).unwrap();
        let rtype = &rec[6..8];
        if rtype != "00" || addr as u32 != want_addr {
            continue;
        }
        let data = &rec[8..8 + 2 * count as usize];
        return (0..count as usize)
            .map(|i| u8::from_str_radix(&data[2 * i..2 * i + 2], 16).unwrap())
            .collect();
    }
    panic!("no data record at address 0x{want_addr:04X} in:\n{hex}");
}

#[test]
fn pic16f877a_matches_gpasm() {
    let hex = gpasm_hex("tests/fixtures/gpasm_config_pic14.asm");
    let bytes = bytes_at(&hex, 0x400E);
    let ours = device::resolve_config(
        &device::PIC16F877A.config,
        "osc=xt, wdt=off, pwrt=on, bor=on, lvp=off, cpd=off, wrt=off, debug=off, cp=off",
    );
    assert_eq!(bytes, ours);
}

#[test]
fn pic18f4550_config4l_matches_gpasm() {
    let hex = gpasm_hex("tests/fixtures/gpasm_config_pic18.asm");
    let bytes = bytes_at(&hex, 0x0006); // low 16 bits of byte address 0x300006
    let ours = device::resolve_config(
        &device::PIC18F4550.config,
        "osc=hspll, plldiv=div5, cpudiv=div1, usbdiv=on, \
         debug=off, xinst=off, icprt=off, lvp=off, stvren=on",
    );
    assert_eq!(&bytes[..1], &ours[6..7]);
}
