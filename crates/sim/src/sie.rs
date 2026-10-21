//! PIC18Fxx5x USB Serial Interface Engine, DS39632E chapter 17.
//!
//! The SIE is the device-side peripheral: it moves packets between the USB
//! bus and the buffer descriptor table, and reports the completed
//! transaction through `USTAT` and `UIR`. Firmware owns the descriptors,
//! the request handling and the endpoint buffers; this module models only
//! the hardware half, so a device stack written against the datasheet runs
//! unchanged.
//!
//! Every piece of SIE state the CPU can see lives in data memory at the
//! addresses the datasheet gives (the registers in bank 15, the buffer
//! descriptor table in USB RAM), so the model is a token-driven view over
//! that memory and the CPU needs no access interception. A host (a test or
//! a gate) issues tokens through [`crate::Pic18`]'s `usb_*` methods.
//!
//! Deliberate limits, all outside endpoint 0 enumeration: ping-pong
//! buffering on EP0 panics rather than mis-simulating, the `USTAT` FIFO is
//! a single latch (a host waits for firmware between tokens), `KEN` is not
//! honoured, the device address is stored but not matched against a bus
//! address, and `PIR2.USBIF` is latched without entering the interrupt
//! vector (the stack polls, as `usb_service` does).

/// `UIR`: USB interrupt status flags, DS39632E register 17-7.
pub const UIR: usize = 0xF68;
/// `UIE`: USB interrupt enables, DS39632E register 17-8.
pub const UIE: usize = 0xF69;
/// `UEIR`: USB error flags, DS39632E register 17-9.
pub const UEIR: usize = 0xF6A;
/// `UEIE`: USB error enables.
pub const UEIE: usize = 0xF6B;
/// `USTAT`: endpoint and direction of the last transaction, DS39632E 17.2.3.
pub const USTAT: usize = 0xF6C;
/// `UCON`: USB control, DS39632E register 17-1.
pub const UCON: usize = 0xF6D;
/// `UADDR`: device address, DS39632E 17.2.5.
pub const UADDR: usize = 0xF6E;
/// `UCFG`: transceiver and ping-pong configuration, DS39632E 17.2.2.
pub const UCFG: usize = 0xF6F;
/// `UEP0`: endpoint 0 control, DS39632E register 17-4. `UEPn` follows at
/// `UEP0 + n`.
pub const UEP0: usize = 0xF70;
/// `PIE2`: peripheral interrupt enables.
pub const PIE2: usize = 0xFA0;
/// `PIR2`: peripheral interrupt flags.
pub const PIR2: usize = 0xFA1;

/// The buffer descriptor table: USB RAM bank 4, DS39632E 17.4.
pub const BDT: usize = 0x400;
/// USB RAM, the only range a buffer descriptor address may point into,
/// DS39632E 17.4.3.
pub const USB_RAM: std::ops::Range<usize> = 0x400..0x800;

/// `UCON<3>`: the module is enabled and the device is attached.
pub const UCON_USBEN: u8 = 0x08;
/// `UCON<4>`: packet processing disabled, set by the SIE on a SETUP token.
pub const UCON_PKTDIS: u8 = 0x10;
/// `UIR<0>`: a valid USB reset was seen and `UADDR` was cleared.
pub const UIR_URSTIF: u8 = 0x01;
/// `UIE<0>`: the USB reset interrupt is enabled.
pub const UIE_URSTIE: u8 = 0x01;
/// `UIE<3>`: the transaction complete interrupt is enabled.
pub const UIE_TRNIE: u8 = 0x08;
/// `UIE<5>`: the stall interrupt is enabled.
pub const UIE_STALLIE: u8 = 0x20;
/// `UIR<3>`: a transaction completed; clearing it advances the USTAT FIFO.
pub const UIR_TRNIF: u8 = 0x08;
/// `UIR<5>`: the SIE answered a STALL handshake.
pub const UIR_STALLIF: u8 = 0x20;
/// `UEPn<0>`: the endpoint issued a STALL.
pub const UEP_EPSTALL: u8 = 0x01;
/// `UEPn<1>`: IN transactions enabled.
pub const UEP_EPINEN: u8 = 0x02;
/// `UEPn<2>`: OUT transactions enabled.
pub const UEP_EPOUTEN: u8 = 0x04;
/// `UEPn<3>`: control (SETUP) transactions disabled while both directions
/// are enabled.
pub const UEP_EPCONDIS: u8 = 0x08;
/// `UEPn<4>`: handshaking enabled.
pub const UEP_EPHSHK: u8 = 0x10;

/// `PIR2<5>`: an unmasked USB flag is set.
pub const PIR2_USBIF: u8 = 0x20;

/// `BDnSTAT<7>`: the SIE owns the descriptor and its buffer.
const BD_UOWN: u8 = 0x80;
/// `BDnSTAT<6>`, CPU mode: the data toggle of the next packet.
const BD_DTS: u8 = 0x40;
/// `BDnSTAT<3>`, CPU mode: check a received packet's data toggle.
const BD_DTSEN: u8 = 0x08;
/// `BDnSTAT<2>`, CPU mode: answer STALL to a token that would use the BD.
const BD_BSTALL: u8 = 0x04;

/// Packet identifiers, `usb_ch9.h`; the SIE writes the token's PID into
/// `BDnSTAT<5:2>`.
pub const PID_OUT: u8 = 0x01;
pub const PID_IN: u8 = 0x09;
pub const PID_SETUP: u8 = 0x0D;

/// One host token's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsbToken {
    /// The SIE accepted the token and moved `data`: device to host for an
    /// IN token, host to device for an OUT or SETUP token. `data1` is the
    /// packet's DATA1 toggle (`false` for DATA0).
    Ack { data: Vec<u8>, data1: bool },
    /// The endpoint is not ready: no buffer armed, packet processing
    /// disabled, or the packet does not fit the armed buffer.
    Nak,
    /// The endpoint answered STALL; the descriptor is left untouched.
    Stall,
}

/// A USB reset (DS39632E 17.2.5): `UADDR` clears, the endpoint stall
/// latches clear, and `URSTIF` asks firmware to re-initialise.
pub fn reset(ram: &mut [u8; 4096]) {
    ram[UADDR] = 0;
    ram[UIR] = (ram[UIR] & !(UIR_TRNIF | UIR_STALLIF)) | UIR_URSTIF;
    // The SIE stops processing packets for the rest of the setup, so a
    // reset that leaves `PKTDIS` set would leave the device deaf until
    // firmware rewrote `UCON`. A bus reset re-arms the module.
    ram[UCON] &= !UCON_PKTDIS;
    raise_usbif(ram, UIR_URSTIF, UIE_URSTIE);
    for ep in 0..16 {
        ram[UEP0 + ep] &= !UEP_EPSTALL;
    }
}

/// A SETUP token: the 8-byte packet lands in endpoint 0's OUT buffer and
/// the SIE takes `PKTDIS` until firmware clears it. Both directions must be
/// enabled for control transfers (DS39632E 17.2.4).
pub fn setup(ram: &mut [u8; 4096], packet: &[u8; 8]) -> UsbToken {
    if !control_enabled(ram) {
        return UsbToken::Nak;
    }
    let out = Bd::load(ram, out_bd(ram));
    if !out.owned() {
        return UsbToken::Nak;
    }
    if out.stalled() {
        return stall(ram);
    }
    if out.space() < packet.len() {
        return UsbToken::Nak;
    }
    if !in_usb_ram(out.addr, packet.len()) {
        return UsbToken::Nak;
    }
    ram[out.addr..out.addr + packet.len()].copy_from_slice(packet);
    out.retire(ram, PID_SETUP, packet.len());
    complete(ram, 0, false);
    ram[UCON] |= UCON_PKTDIS;
    UsbToken::Ack {
        data: packet.to_vec(),
        data1: false,
    }
}

/// An IN token: send the packet endpoint 0's IN descriptor holds, or STALL
/// when the descriptor was armed with `BSTALL`. The descriptor's `DTS` bit
/// is the toggle to send (DS39632E register 17-5).
pub fn in_token(ram: &mut [u8; 4096]) -> UsbToken {
    if !direction_enabled(ram, UEP_EPINEN) {
        return UsbToken::Nak;
    }
    let inbd = Bd::load(ram, in_bd(ram));
    if !inbd.owned() {
        return UsbToken::Nak;
    }
    if inbd.stalled() {
        return stall(ram);
    }
    let len = inbd.space();
    if !in_usb_ram(inbd.addr, len) {
        return UsbToken::Nak;
    }
    let data = ram[inbd.addr..inbd.addr + len].to_vec();
    let data1 = inbd.stat & BD_DTS != 0;
    inbd.retire(ram, PID_IN, len);
    complete(ram, 0, true);
    UsbToken::Ack { data, data1 }
}

/// An OUT token: store the packet in endpoint 0's OUT buffer. A packet
/// larger than the armed buffer is not stored (DS39632E 17.4.2); one whose
/// toggle disagrees with an armed `DTSEN` is acknowledged but ignored
/// (DS39632E table 17-3).
pub fn out_token(ram: &mut [u8; 4096], data: &[u8], data1: bool) -> UsbToken {
    if !direction_enabled(ram, UEP_EPOUTEN) {
        return UsbToken::Nak;
    }
    let out = Bd::load(ram, out_bd(ram));
    if !out.owned() {
        return UsbToken::Nak;
    }
    if out.stalled() {
        return stall(ram);
    }
    if data.len() > out.space() {
        return UsbToken::Nak;
    }
    if out.stat & BD_DTSEN != 0 && data1 != (out.stat & BD_DTS != 0) {
        // Acknowledged but not stored: the payload never reached the
        // buffer, so there is none to hand back.
        return UsbToken::Ack {
            data: Vec::new(),
            data1,
        };
    }
    if !in_usb_ram(out.addr, data.len()) {
        return UsbToken::Nak;
    }
    ram[out.addr..out.addr + data.len()].copy_from_slice(data);
    out.retire(ram, PID_OUT, data.len());
    complete(ram, 0, false);
    UsbToken::Ack {
        data: data.to_vec(),
        data1,
    }
}

/// A buffer descriptor as the CPU left it, or as the SIE returns it
/// (DS39632E 17.4.1). Both views share `UOWN` and `BC9:BC8`; the meaning of
/// bits 6:2 depends on who owns it.
struct Bd {
    index: usize,
    stat: u8,
    cnt: u8,
    addr: usize,
}

impl Bd {
    fn load(ram: &[u8; 4096], index: usize) -> Self {
        let base = BDT + 4 * index;
        Bd {
            index,
            stat: ram[base],
            cnt: ram[base + 1],
            addr: (ram[base + 2] as usize) | ((ram[base + 3] as usize) << 8),
        }
    }

    fn owned(&self) -> bool {
        self.stat & BD_UOWN != 0
    }

    fn stalled(&self) -> bool {
        self.stat & BD_BSTALL != 0
    }

    /// The 10-bit byte count: `BDnCNT` plus `BDnSTAT<1:0>` (DS39632E 17.4.2).
    fn space(&self) -> usize {
        self.cnt as usize | ((self.stat as usize & 0x03) << 8)
    }

    /// Hand the descriptor back with the SIE's PID and byte count written
    /// in (DS39632E register 17-6), leaving the buffer address alone.
    fn retire(&self, ram: &mut [u8; 4096], pid: u8, len: usize) {
        let base = BDT + 4 * self.index;
        ram[base] = ((pid & 0x0F) << 2) | ((len >> 8) as u8 & 0x03);
        ram[base + 1] = len as u8;
        ram[base + 2] = self.addr as u8;
        ram[base + 3] = (self.addr >> 8) as u8;
    }
}

/// Endpoint 0's OUT and IN buffer descriptor indices for the ping-pong mode
/// in `UCFG<1:0>`, DS39632E table 17-4. Modes 1 and 2 ping-pong EP0, which
/// this model does not serve.
fn ep0_bds(ram: &[u8; 4096]) -> (usize, usize) {
    match ram[UCFG] & 0x03 {
        0x00 | 0x03 => (0, 1),
        mode => panic!("sim(pic18): USB ping-pong on EP0 is not modelled (UCFG.PPB={mode:#04b})"),
    }
}

fn out_bd(ram: &[u8; 4096]) -> usize {
    ep0_bds(ram).0
}

fn in_bd(ram: &[u8; 4096]) -> usize {
    ep0_bds(ram).1
}

/// `UCON.USBEN` set and packet processing not disabled by a pending SETUP.
fn module_listening(ram: &[u8; 4096]) -> bool {
    let ucon = ram[UCON];
    ucon & UCON_USBEN != 0 && ucon & UCON_PKTDIS == 0
}

/// A SETUP needs both directions enabled with `EPCONDIS` clear (17.2.4).
fn control_enabled(ram: &[u8; 4096]) -> bool {
    if !module_listening(ram) {
        return false;
    }
    let uep = ram[UEP0];
    uep & UEP_EPHSHK != 0
        && uep & UEP_EPINEN != 0
        && uep & UEP_EPOUTEN != 0
        && uep & UEP_EPCONDIS == 0
}

/// An IN or OUT transaction needs handshaking plus that direction enabled.
fn direction_enabled(ram: &[u8; 4096], direction: u8) -> bool {
    module_listening(ram) && ram[UEP0] & UEP_EPHSHK != 0 && ram[UEP0] & direction != 0
}

/// A buffer descriptor address must land in USB RAM (DS39632E 17.4.3).
fn in_usb_ram(addr: usize, len: usize) -> bool {
    USB_RAM.contains(&addr) && USB_RAM.contains(&(addr + len.saturating_sub(1)))
}

/// Latch the transaction status and raise the token interrupt.
fn complete(ram: &mut [u8; 4096], ep: u8, dir_in: bool) {
    ram[USTAT] = ((ep & 0x0F) << 3) | if dir_in { 0x04 } else { 0 };
    ram[UIR] |= UIR_TRNIF;
    raise_usbif(ram, UIR_TRNIF, UIE_TRNIE);
}

/// Raise the top-level USB interrupt flag when an enabled source fired
/// (DS39632E figure 17-8: every `UIR` flag is ANDed with its `UIE` enable
/// before the OR into `PIR2.USBIF`).
fn raise_usbif(ram: &mut [u8; 4096], flag: u8, enable: u8) {
    if ram[UIE] & enable != 0 {
        ram[PIR2] |= PIR2_USBIF;
    }
    let _ = flag;
}

/// Answer STALL: the endpoint latch and flag rise, and the armed descriptor
/// keeps both its contents and `UOWN` (DS39632E 17.4.1.2). Real PIC18
/// silicon does not clear the latch on the next SETUP either, which is why
/// a stack clears it in software.
fn stall(ram: &mut [u8; 4096]) -> UsbToken {
    ram[UEP0] |= UEP_EPSTALL;
    ram[UIR] |= UIR_STALLIF;
    raise_usbif(ram, UIR_STALLIF, UIE_STALLIE);
    UsbToken::Stall
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh endpoint zero: module on, both directions on, EP0 OUT armed
    /// with an 8-byte buffer at 0x500 and EP0 IN pointed at 0x508.
    fn endpoint0() -> [u8; 4096] {
        let mut ram = [0u8; 4096];
        ram[UCON] = UCON_USBEN;
        ram[UEP0] = UEP_EPHSHK | UEP_EPINEN | UEP_EPOUTEN;
        arm_out(&mut ram, 8, 0x500);
        ram[0x404 + 1] = 8;
        ram[0x404 + 2] = 0x08;
        ram[0x404 + 3] = 0x05;
        ram
    }

    fn arm_out(ram: &mut [u8; 4096], len: u8, addr: usize) {
        ram[BDT + 1] = len;
        ram[BDT + 2] = addr as u8;
        ram[BDT + 3] = (addr >> 8) as u8;
        ram[BDT] = BD_UOWN;
    }

    fn arm_in(ram: &mut [u8; 4096], len: u8, data1: bool) {
        arm_in_at(ram, len, data1, 0x508);
    }

    fn arm_in_at(ram: &mut [u8; 4096], len: u8, data1: bool, addr: usize) {
        ram[0x404 + 1] = len;
        ram[0x404 + 2] = addr as u8;
        ram[0x404 + 3] = (addr >> 8) as u8;
        ram[0x404] = BD_UOWN | BD_DTSEN | if data1 { BD_DTS } else { 0 };
    }

    #[test]
    fn setup_lands_in_the_out_buffer_and_raises_the_token_flag() {
        let mut ram = endpoint0();
        let pkt = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00];
        assert_eq!(
            setup(&mut ram, &pkt),
            UsbToken::Ack {
                data: pkt.to_vec(),
                data1: false
            }
        );
        assert_eq!(&ram[0x500..0x508], &pkt);
        assert_eq!(ram[BDT] & 0x7C, PID_SETUP << 2, "the SIE wrote the PID");
        assert_eq!(ram[BDT] & BD_UOWN, 0, "the CPU owns the descriptor again");
        assert_eq!(ram[BDT + 1], 8, "byte count");
        assert_eq!(ram[USTAT] & 0x78, 0, "endpoint 0");
        assert_eq!(ram[USTAT] & 0x04, 0, "OUT direction");
        assert_eq!(ram[UIR] & UIR_TRNIF, UIR_TRNIF);
        assert_eq!(
            ram[UCON] & UCON_PKTDIS,
            UCON_PKTDIS,
            "SETUP disables tokens"
        );
        assert_eq!(
            ram[PIR2] & PIR2_USBIF,
            0,
            "an unmasked source raises no flag"
        );
    }

    #[test]
    fn setup_is_nakked_until_firmware_clears_packet_disable() {
        let mut ram = endpoint0();
        let pkt = [0x00, 0x05, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(matches!(setup(&mut ram, &pkt), UsbToken::Ack { .. }));
        assert_eq!(setup(&mut ram, &pkt), UsbToken::Nak);
        ram[UCON] &= !UCON_PKTDIS;
        arm_out(&mut ram, 8, 0x500);
        assert!(matches!(setup(&mut ram, &pkt), UsbToken::Ack { .. }));
    }

    #[test]
    fn setup_is_nakked_without_an_armed_out_descriptor() {
        let mut ram = endpoint0();
        ram[BDT] = 0;
        assert_eq!(setup(&mut ram, &[0u8; 8]), UsbToken::Nak);
    }

    #[test]
    fn in_token_sends_the_armed_packet_with_its_descriptor_toggle() {
        let mut ram = endpoint0();
        ram[0x508] = 0x12;
        ram[0x509] = 0x01;
        arm_in(&mut ram, 2, false);
        match in_token(&mut ram) {
            UsbToken::Ack { data, data1 } => {
                assert_eq!(data, vec![0x12, 0x01]);
                assert!(!data1, "DATA0");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(ram[0x404] & 0x7C, PID_IN << 2);
        assert_eq!(ram[0x404] & BD_UOWN, 0);
        assert_eq!(ram[0x404 + 1], 2, "byte count");
        assert_eq!(ram[USTAT] & 0x04, 0x04, "IN direction");
    }

    #[test]
    fn in_token_naks_until_the_cpu_arms_the_descriptor() {
        let mut ram = endpoint0();
        assert_eq!(in_token(&mut ram), UsbToken::Nak);
        arm_in(&mut ram, 1, true);
        assert!(matches!(in_token(&mut ram), UsbToken::Ack { .. }));
    }

    #[test]
    fn a_bstall_descriptor_answers_stall_and_stays_owned() {
        let mut ram = endpoint0();
        ram[0x404] = BD_UOWN | BD_BSTALL;
        ram[0x404 + 1] = 0;
        assert_eq!(in_token(&mut ram), UsbToken::Stall);
        assert_eq!(ram[0x404], BD_UOWN | BD_BSTALL, "descriptor untouched");
        assert_eq!(ram[UEP0] & UEP_EPSTALL, UEP_EPSTALL);
        assert_eq!(ram[UIR] & UIR_STALLIF, UIR_STALLIF);
        assert_eq!(ram[UIR] & UIR_TRNIF, 0, "a stalled token is no completion");
        assert_eq!(
            ram[PIR2] & PIR2_USBIF,
            0,
            "an unmasked source raises no flag"
        );
    }

    #[test]
    fn out_token_stores_and_retires_the_descriptor() {
        let mut ram = endpoint0();
        let pkt = [0x00, 0xC2, 0x01, 0x00, 0x00, 0x00, 0x08];
        assert!(matches!(
            out_token(&mut ram, &pkt, true),
            UsbToken::Ack { .. }
        ));
        assert_eq!(&ram[0x500..0x507], &pkt);
        assert_eq!(ram[BDT] & 0x7C, PID_OUT << 2);
        assert_eq!(ram[BDT + 1], 7);
        assert_eq!(ram[USTAT] & 0x04, 0, "OUT direction");
    }

    #[test]
    fn out_token_larger_than_the_armed_buffer_is_nakked_and_keeps_the_count() {
        let mut ram = endpoint0();
        arm_out(&mut ram, 4, 0x500);
        assert_eq!(out_token(&mut ram, &[1, 2, 3, 4, 5], true), UsbToken::Nak);
        assert_eq!(ram[BDT + 1], 4, "byte count unchanged");
        assert_eq!(ram[BDT] & BD_UOWN, BD_UOWN, "still armed");
    }

    #[test]
    fn out_token_with_a_mismatched_toggle_is_acknowledged_but_ignored() {
        let mut ram = endpoint0();
        ram[BDT] = BD_UOWN | BD_DTSEN; /* expect DATA0 */
        ram[BDT + 1] = 8;
        assert!(matches!(
            out_token(&mut ram, &[0xAA], true),
            UsbToken::Ack { .. }
        ));
        assert_eq!(ram[BDT + 1], 8, "count untouched");
        assert_eq!(ram[BDT] & BD_UOWN, BD_UOWN, "still armed");
        assert_eq!(ram[UIR] & UIR_TRNIF, 0, "no transaction completed");
    }

    #[test]
    fn a_descriptor_pointing_outside_usb_ram_is_refused() {
        // A buffer descriptor address outside USB RAM loses the data on
        // real silicon (DS39632E 17.4.3), and a linker that scatters the
        // endpoint buffers into the ordinary GPR banks produces exactly
        // that. The SIE cannot move a packet to or from such a buffer.
        let mut ram = endpoint0();
        arm_in_at(&mut ram, 8, false, 0x0200);
        assert_eq!(in_token(&mut ram), UsbToken::Nak, "no buffer in USB RAM");

        let mut ram = endpoint0();
        arm_out(&mut ram, 8, 0x0200);
        let pkt = [0x00u8; 8];
        assert_eq!(setup(&mut ram, &pkt), UsbToken::Nak, "no buffer in USB RAM");
    }

    #[test]
    fn usb_reset_clears_the_address_and_every_endpoint_stall() {
        let mut ram = endpoint0();
        ram[UADDR] = 5;
        ram[UEP0] |= UEP_EPSTALL;
        ram[UEP0 + 3] |= UEP_EPSTALL;
        ram[UIR] = UIR_TRNIF;
        reset(&mut ram);
        assert_eq!(ram[UADDR], 0);
        assert_eq!(ram[UIR] & UIR_URSTIF, UIR_URSTIF);
        assert_eq!(ram[UIR] & UIR_TRNIF, 0);
        assert_eq!(ram[UEP0] & UEP_EPSTALL, 0);
        assert_eq!(ram[UEP0 + 3] & UEP_EPSTALL, 0);
    }

    #[test]
    fn a_bus_reset_leaves_the_module_able_to_answer_again() {
        let mut ram = endpoint0();
        let pkt = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00];
        assert!(matches!(setup(&mut ram, &pkt), UsbToken::Ack { .. }));
        assert_eq!(ram[UCON] & UCON_PKTDIS, UCON_PKTDIS);
        reset(&mut ram);
        assert_eq!(ram[UCON] & UCON_PKTDIS, 0, "the reset re-arms the module");
        arm_out(&mut ram, 8, 0x500);
        assert!(
            matches!(setup(&mut ram, &pkt), UsbToken::Ack { .. }),
            "the device is enumerable again after a bus reset"
        );
    }

    #[test]
    fn the_top_level_flag_follows_the_enable_bits() {
        let mut ram = endpoint0();
        // No enable: the flag in UIR rises, the top-level flag does not.
        arm_in(&mut ram, 1, false);
        assert!(matches!(in_token(&mut ram), UsbToken::Ack { .. }));
        assert_eq!(ram[UIR] & UIR_TRNIF, UIR_TRNIF);
        assert_eq!(
            ram[PIR2] & PIR2_USBIF,
            0,
            "an unmasked source raises no flag"
        );

        // With the enable set, the top-level flag follows.
        ram[UIE] = UIE_TRNIE;
        arm_in(&mut ram, 1, false);
        assert!(matches!(in_token(&mut ram), UsbToken::Ack { .. }));
        assert_eq!(ram[PIR2] & PIR2_USBIF, PIR2_USBIF);
    }

    #[test]
    fn an_ignored_but_acknowledged_packet_hands_back_no_data() {
        let mut ram = endpoint0();
        ram[BDT] = BD_UOWN | BD_DTSEN; /* expect DATA0 */
        ram[BDT + 1] = 8;
        match out_token(&mut ram, &[0xAA, 0xBB], true) {
            UsbToken::Ack { data, .. } => assert!(data.is_empty(), "nothing was stored"),
            other => panic!("{other:?}"),
        }
        assert_eq!(&ram[0x500..0x502], &[0x00, 0x00], "the buffer is untouched");
    }

    #[test]
    fn a_ping_pong_ep0_configuration_panics_rather_than_mis_simulating() {
        let mut ram = endpoint0();
        ram[UCFG] = 0x01;
        let pkt = [0u8; 8];
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| setup(&mut ram, &pkt)))
                .is_err()
        );
    }
}
