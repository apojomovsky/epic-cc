//! USB CDC-ACM enumeration through the PIC18 SIE model (epic-cc#1416).
//!
//! The device under test is a PIC18F4550 program speaking M-Stack's
//! endpoint 0 protocol (crates/driver/tests/fixtures/usb-ep0/usb_ep0_cdc.c),
//! compiled by the real `epic-cc` binary and run on the `Pic18` simulator.
//! The host half below issues SETUP, IN and OUT tokens against
//! `crates/sim`'s SIE model and walks the enumeration a CDC host performs.
//! Every stage names itself in a failure, so a stall at the wrong stage
//! reads as such instead of as a generic timeout.

use pic14_sim::sie::{
    UsbToken, UADDR, UCON, UCON_PKTDIS, UCON_USBEN, UEP0, UIR, UIR_STALLIF, UIR_TRNIF, UIR_URSTIF,
};

/// The descriptor bytes below are a hand-synced copy of the ones epic-hal's
/// `lib/usb` ships (`lib/usb/src/usb_descriptors.c`): the device descriptor
/// here, the configuration descriptor, and the four string descriptors. A
/// change to epic-hal's copies does not reach this file, so the two drift
/// apart unless someone syncs them.
///
/// The device descriptor `lib/usb` ships: USB 2.0, CDC-ACM, EP0 packet size
/// 8, VID 0xA0A0 / PID 0x0004, one configuration.
const DEVICE_DESC: [u8; 18] = [
    18, 0x01, 0x00, 0x02, 0xEF, 0x02, 0x01, 0x08, 0xA0, 0xA0, 0x04, 0x00, 0x01, 0x00, 1, 2, 3, 1,
];

/// The configuration descriptor: config + IAD + CDC comm interface with the
/// header/ACM/union functional descriptors and the notification endpoint,
/// then the data interface with the bulk pair.
const CONFIG_DESC: [u8; 70] = [
    9, 0x02, 70, 0, 2, 1, 0, 0x80, 50, // configuration
    8, 0x0B, 0, 2, 0x02, 0x02, 0, 0, // interface association
    9, 0x04, 0, 0, 1, 0x02, 0x02, 0, 0, // comm interface
    5, 0x24, 0x00, 0x10, 0x01, // functional header, bcdCDC 1.10
    4, 0x24, 0x02, 0x06, // ACM: line codings and send break
    5, 0x24, 0x06, 0, 1, // union: comm 0, data 1
    7, 0x05, 0x81, 0x03, 10, 0, 1, // notification endpoint, EP1 IN
    9, 0x04, 1, 0, 2, 0x0A, 0, 0, 0, // data interface
    7, 0x05, 0x82, 0x02, 64, 0, 1, // bulk IN, EP2
    7, 0x05, 0x02, 0x02, 64, 0, 1, // bulk OUT, EP2
];

/// The language table, then the vendor, product and serial strings
/// `lib/usb` ships.
fn string_descs() -> [Vec<u8>; 4] {
    [
        vec![4, 0x03, 0x09, 0x04],
        utf16("pic8-usb project"),
        utf16("pic8-usb"),
        utf16("DEV BUILD - NOT FOR SHIPPING."),
    ]
}

/// A UTF-16LE string descriptor of the length its characters need.
fn utf16(s: &str) -> Vec<u8> {
    let mut out = vec![(2 + 2 * s.len()) as u8, 0x03];
    for b in s.bytes() {
        out.push(b);
        out.push(0);
    }
    out
}

/// The map's `global <name> 0xNN` address, as the driver prints it.
fn map_addr(map: &str, name: &str) -> usize {
    let prefix = format!("global {name} 0x");
    let line = map
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no map entry for {name} in:\n{map}"));
    usize::from_str_radix(line[prefix.len()..].trim(), 16).expect("map address is hex")
}

/// The device program, its RAM image and the names the walk reads back.
struct Device {
    sim: pic14_sim::Pic18,
    map: String,
}

/// How long the walk waits for firmware to answer a token before calling it
/// a stall. The device program's loop is a handful of instructions, so a
/// few thousand steps is generous; the bound is what turns "the model never
/// completed the token" into a named failure instead of a hang.
const SERVICE_STEPS: usize = 20_000;

/// The host's view of a control transfer's data stage.
#[derive(Debug, PartialEq, Eq)]
enum StageResult {
    /// The device's packet, and the DATA1 toggle it sent it with.
    Data {
        data: Vec<u8>,
        data1: bool,
    },
    Stalled,
}

impl Device {
    fn build(tag: &str) -> Self {
        let fixture = "tests/fixtures/usb-ep0/usb_ep0_cdc.c";
        let stem = std::env::temp_dir().join(format!("usb_ep0_{tag}_{}", std::process::id()));
        let hex_path = stem.with_extension("hex");
        let map_path = stem.with_extension("map");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_epic-cc"))
            .args(["--target", "18F4550", "-o"])
            .arg(&hex_path)
            .arg("--map")
            .arg(&map_path)
            .arg(fixture)
            .output()
            .expect("run epic-cc");
        assert!(
            out.status.success(),
            "epic-cc on {fixture}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let hex = std::fs::read_to_string(&hex_path).expect("read hex");
        let map = std::fs::read_to_string(&map_path).expect("read map");
        let _ = std::fs::remove_file(&hex_path);
        let _ = std::fs::remove_file(&map_path);
        Device {
            sim: pic14_sim::Pic18::new(pic14_sim::parse_hex_pic18(&hex)),
            map,
        }
    }

    fn ram(&self, name: &str) -> u8 {
        self.sim.ram()[map_addr(&self.map, name)]
    }

    /// Let the device program run until it has consumed the token the SIE
    /// latched, i.e. until it clears `TRNIF` (M-Stack's `usb_service`), and
    /// then until it has armed the next response.
    fn service(&mut self, stage: &str) {
        for _ in 0..SERVICE_STEPS {
            if self.sim.ram()[UIR] & UIR_TRNIF == 0 {
                // One more pass so the arm writes land after the clear.
                self.sim.run(64);
                return;
            }
            self.sim.step();
        }
        panic!("{stage}: the device did not service the token within {SERVICE_STEPS} steps");
    }

    /// Let the device program reach a running state: the module enabled and
    /// its buffers armed.
    fn warm_up(&mut self) {
        for _ in 0..SERVICE_STEPS {
            if self.sim.ram()[UCON] & UCON_USBEN != 0 {
                self.sim.run(256);
                return;
            }
            self.sim.step();
        }
        panic!("the device did not enable the USB module within {SERVICE_STEPS} steps");
    }

    /// A bus reset, then the device re-initialises itself from `URSTIF`.
    /// The wait is for the reset to be consumed, not for the device to be up:
    /// the device was already up before the reset.
    fn bus_reset(&mut self) {
        self.sim.usb_reset();
        assert_eq!(
            self.sim.ram()[UIR] & UIR_URSTIF,
            UIR_URSTIF,
            "the reset raises URSTIF"
        );
        for _ in 0..SERVICE_STEPS {
            if self.sim.ram()[UIR] & UIR_URSTIF == 0 {
                self.sim.run(256);
                return;
            }
            self.sim.step();
        }
        panic!("the device did not consume the bus reset within {SERVICE_STEPS} steps");
    }

    /// Issue one IN token, letting the device arm its descriptor first. A
    /// stalled endpoint is reported to the caller, a token the device never
    /// arms is a named failure.
    fn in_token(&mut self, stage: &str) -> StageResult {
        self.try_in_token(stage)
            .unwrap_or_else(|| panic!("{stage}: the device never armed an IN packet"))
    }

    /// Issue one IN token, or report that the device has nothing armed.
    /// The end of a data stage looks like this: the device answered the
    /// last packet with a short one and queued nothing after it.
    fn try_in_token(&mut self, stage: &str) -> Option<StageResult> {
        for _ in 0..64 {
            match self.sim.usb_in() {
                UsbToken::Ack { data, data1 } => {
                    self.service(stage);
                    return Some(StageResult::Data { data, data1 });
                }
                UsbToken::Stall => {
                    self.service(stage);
                    return Some(StageResult::Stalled);
                }
                UsbToken::Nak => {
                    self.sim.run(64);
                }
            }
        }
        None
    }

    /// Issue one OUT token, retrying while the device has not armed the
    /// buffer yet.
    fn out_token(&mut self, stage: &str, data: &[u8], data1: bool) {
        for _ in 0..64 {
            match self.sim.usb_out(data, data1) {
                UsbToken::Ack { .. } => {
                    self.service(stage);
                    return;
                }
                UsbToken::Stall => panic!("{stage}: EP0 stalled on an OUT packet"),
                UsbToken::Nak => {
                    self.sim.run(64);
                }
            }
        }
        panic!("{stage}: the device never armed an OUT buffer");
    }

    /// The setup stage. The first SETUP after a reset is issued while the
    /// device may still be arming its OUT buffer, so retry the token itself.
    fn setup(&mut self, stage: &str, packet: [u8; 8]) {
        for _ in 0..64 {
            match self.sim.usb_setup(&packet) {
                UsbToken::Ack { .. } => {
                    self.service(stage);
                    return;
                }
                UsbToken::Stall => panic!("{stage}: EP0 stalled on the SETUP token"),
                UsbToken::Nak => {
                    self.sim.run(64);
                }
            }
        }
        panic!("{stage}: the device never armed the SETUP buffer");
    }

    /// A control read: setup, the IN data stage, then the zero-length OUT
    /// that acknowledges it. Fails by name if EP0 stalls at either stage.
    fn control_read(&mut self, stage: &str, request: [u8; 8], want: &[u8]) -> Vec<u8> {
        self.setup(stage, request);
        let mut got = Vec::new();
        let max = u16::from_le_bytes([request[6], request[7]]) as usize;
        // A control transfer's data stage starts with DATA1 and then
        // alternates (USB 2.0 section 8.5.3); the host tracks the toggle,
        // so a device that sends the wrong one is a protocol error rather
        // than a host detail.
        let mut expect_data1 = true;
        loop {
            match self.try_in_token(stage) {
                Some(StageResult::Data { data, data1 }) => {
                    if data1 != expect_data1 {
                        panic!(
                            "{stage}: the device sent DATA{} where DATA{} was due",
                            data1 as u8, expect_data1 as u8
                        );
                    }
                    expect_data1 = !expect_data1;
                    if data.is_empty() {
                        // The zero-length packet ends the data stage.
                        break;
                    }
                    got.extend_from_slice(&data);
                    if got.len() >= max || got.len() >= want.len() {
                        break;
                    }
                }
                Some(StageResult::Stalled) => {
                    panic!("{stage}: EP0 stalled in the data stage")
                }
                None => panic!("{stage}: the device stopped answering mid-transfer"),
            }
        }
        // The status stage of a control read is a zero-length OUT packet.
        self.out_token(stage, &[], true);
        assert_eq!(got, want, "{stage}: the device's answer");
        got
    }
}

/// `bmRequestType` for a standard, device-to-host request.
const IN_STD: u8 = 0x80;
/// `bmRequestType` for a CDC class, host-to-device request.
const OUT_CLASS: u8 = 0x21;
/// `bmRequestType` for a CDC class, device-to-host request.
const IN_CLASS: u8 = 0xA1;

fn setup_packet(bm: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let v = value.to_le_bytes();
    let i = index.to_le_bytes();
    let l = length.to_le_bytes();
    [bm, request, v[0], v[1], i[0], i[1], l[0], l[1]]
}

#[test]
fn cdc_acm_enumeration_walks_every_endpoint_zero_stage() {
    let mut dev = Device::build("enumerate");
    dev.warm_up();
    dev.bus_reset();

    // The device descriptor: the host learns EP0's packet size from it, so
    // it is the first request of every enumeration.
    let got = dev.control_read(
        "GET_DESCRIPTOR(DEVICE)",
        setup_packet(IN_STD, 0x06, 0x0100, 0, 64),
        &DEVICE_DESC,
    );
    assert_eq!(got.len(), 18, "the device descriptor is not truncated");
    assert_eq!(got[7], 8, "bMaxPacketSize0 is 8");
    assert_eq!([got[8], got[9]], [0xA0, 0xA0], "VID");
    assert_eq!([got[10], got[11]], [0x04, 0x00], "PID");

    // SET_ADDRESS: the address takes effect after the status stage.
    dev.setup("SET_ADDRESS", setup_packet(0x00, 0x05, 5, 0, 0));
    assert_eq!(
        dev.in_token("SET_ADDRESS status"),
        StageResult::Data {
            data: vec![],
            data1: true
        }
    );
    assert_eq!(dev.sim.ram()[UADDR], 5, "UADDR takes the new address");

    // The configuration descriptor is nine packets of eight bytes plus the
    // tail, the multi-packet data stage enumeration has to get right.
    let got = dev.control_read(
        "GET_DESCRIPTOR(CONFIG)",
        setup_packet(IN_STD, 0x06, 0x0200, 0, 255),
        &CONFIG_DESC,
    );
    assert_eq!(got.len(), 70, "wTotalLength bytes");
    assert_eq!(
        [got[0], got[1]],
        [9, 0x02],
        "configuration descriptor header"
    );
    assert_eq!(got[2] as usize, got.len(), "wTotalLength matches the bytes");
    assert_eq!(got[4], 2, "two interfaces");
    assert_eq!(
        [got[9], got[10], got[11]],
        [8, 0x0B, 0],
        "IAD first interface"
    );
    assert_eq!([got[17], got[18], got[21]], [9, 0x04, 1], "comm interface");
    assert_eq!([got[26], got[27], got[28]], [5, 0x24, 0x00], "CDC header");
    assert_eq!(
        [got[40], got[41], got[42]],
        [7, 0x05, 0x81],
        "notification EP"
    );
    assert_eq!([got[47], got[48], got[51]], [9, 0x04, 2], "data interface");
    assert_eq!([got[56], got[57], got[58]], [7, 0x05, 0x82], "bulk IN");
    assert_eq!([got[63], got[64], got[65]], [7, 0x05, 0x02], "bulk OUT");

    // The string descriptors, including the language table.
    let strings = string_descs();
    for (index, want) in strings.iter().enumerate() {
        let stage = format!("GET_DESCRIPTOR(STRING {index})");
        let request = setup_packet(IN_STD, 0x06, 0x0300 | index as u16, 0, 255);
        let got = dev.control_read(&stage, request, want);
        assert_eq!(&got, want, "{stage}");
    }

    // SET_CONFIGURATION: value 1, then the status ZLP.
    dev.setup("SET_CONFIGURATION", setup_packet(0x00, 0x09, 1, 0, 0));
    assert_eq!(
        dev.in_token("SET_CONFIGURATION status"),
        StageResult::Data {
            data: vec![],
            data1: true
        }
    );
    assert_eq!(
        dev.ram("g_configured"),
        1,
        "the device recorded configuration 1"
    );

    // GET_CONFIGURATION returns what was set.
    let got = dev.control_read(
        "GET_CONFIGURATION",
        setup_packet(IN_STD, 0x08, 0, 0, 1),
        &[1],
    );
    assert_eq!(got, vec![1]);

    // GET_STATUS is two zero bytes for a self-powered device that is not
    // remote wake-up capable.
    let got = dev.control_read("GET_STATUS", setup_packet(IN_STD, 0x00, 0, 0, 2), &[0, 0]);
    assert_eq!(got, vec![0, 0]);

    // CDC SET_LINE_CODING: an OUT data stage of seven bytes, then an IN
    // status stage. 115200 8N1.
    dev.setup("SET_LINE_CODING", setup_packet(OUT_CLASS, 0x20, 0, 0, 7));
    dev.out_token(
        "SET_LINE_CODING data",
        &[0x00, 0xC2, 0x01, 0x00, 0x00, 0x00, 0x08],
        true,
    );
    assert_eq!(
        dev.in_token("SET_LINE_CODING status"),
        StageResult::Data {
            data: vec![],
            data1: true
        },
        "the status stage is a zero-length IN"
    );

    let got = dev.control_read(
        "GET_LINE_CODING",
        setup_packet(IN_CLASS, 0x21, 0, 0, 7),
        &[0x00, 0xC2, 0x01, 0x00, 0x00, 0x00, 0x08],
    );
    assert_eq!(got, vec![0x00, 0xC2, 0x01, 0x00, 0x00, 0x00, 0x08]);

    // CDC SET_CONTROL_LINE_STATE: DTR asserted opens the port, which is what
    // `epic_usb_connected()` reports.
    dev.setup(
        "SET_CONTROL_LINE_STATE",
        setup_packet(OUT_CLASS, 0x22, 0x0003, 0, 0),
    );
    assert_eq!(
        dev.in_token("SET_CONTROL_LINE_STATE status"),
        StageResult::Data {
            data: vec![],
            data1: true
        }
    );
    assert_eq!(dev.ram("g_dtr"), 1, "DTR latched");

    assert_eq!(dev.ram("g_ep0_stalls"), 0, "enumeration stalled EP0");

    // Nothing above may have left the endpoint stalled or the module deaf.
    assert_eq!(dev.sim.ram()[UEP0] & 0x01, 0, "EP0 is not stalled");
    assert_eq!(dev.sim.ram()[UIR] & UIR_STALLIF, 0);
    assert_eq!(dev.sim.ram()[UCON] & UCON_PKTDIS, 0);
}

#[test]
fn an_unsupported_request_stalls_at_the_data_stage_and_the_walk_sees_it() {
    let mut dev = Device::build("stall");
    dev.warm_up();
    dev.bus_reset();

    // A GET_DESCRIPTOR for a descriptor type the device does not have: the
    // stack answers STALL, and the host must see it at the data stage rather
    // than time out. This is the failure the enumeration walk is built to
    // report by name.
    dev.setup(
        "GET_DESCRIPTOR(unknown)",
        setup_packet(IN_STD, 0x06, 0x0F00, 0, 255),
    );
    assert_eq!(
        dev.in_token("GET_DESCRIPTOR(unknown) data"),
        StageResult::Stalled,
        "an unknown descriptor type stalls EP0"
    );
    assert_eq!(dev.ram("g_ep0_stalls"), 1);

    // The stall is recoverable the way M-Stack recovers it: a new SETUP
    // clears the condition in software, and the next transfer succeeds.
    dev.out_token("stall recovery", &[], true);
    dev.setup(
        "GET_DESCRIPTOR(DEVICE) after a stall",
        setup_packet(IN_STD, 0x06, 0x0100, 0, 18),
    );
    match dev.in_token("device descriptor after a stall") {
        StageResult::Data { data, .. } => assert_eq!(data, &DEVICE_DESC[..8]),
        StageResult::Stalled => panic!("EP0 stayed stalled after a fresh SETUP"),
    }
}
