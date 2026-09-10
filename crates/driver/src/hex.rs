//! Intel HEX emission for the driver's final stage: program words plus the
//! device's config bytes, per core. The PIC14 path writes one config word
//! into the program image (its config word sits just past the flash ceiling,
//! so the image is resized to include it); PIC18 and PIC14E write config
//! words as a separate region through `asm::to_hex_regions`, which emits
//! the `:04` extended-linear-address record a config base above 0xFFFF
//! needs (PIC14E's CONFIG1/CONFIG2 live at byte 0x1000E, docs/33 §3).

use device::{Core, Device};

/// Render the final Intel HEX for `program_words` plus `config_bytes`.
///
/// `config_bytes` is `None` when the source named no `EPIC_CONFIG`, in
/// which case the program image is emitted alone. The PIC14 arm keeps the
/// historical shape: the single config word is spliced into the program
/// word array at `base_byte_addr / 2` and the whole image goes through
/// `asm::to_hex`, so existing PIC14 HEX output is byte-identical.
pub fn emit(device: &Device, program_words: &[u16], config_bytes: Option<&[u8]>) -> String {
    match (device.core, config_bytes) {
        (Core::Pic14, Some(cb)) => {
            let mut words = program_words.to_vec();
            let idx = (device.config.base_byte_addr / 2) as usize;
            if words.len() <= idx {
                words.resize(idx + 1, 0);
            }
            let w = u16::from(cb[0]) | (u16::from(cb[1]) << 8);
            words[idx] = w;
            asm::to_hex(&words)
        }
        (Core::Pic18 | Core::Pic14e, Some(cb)) => {
            let mut config_words = Vec::new();
            for chunk in cb.chunks(2) {
                let lo = chunk[0] as u16;
                let hi = if chunk.len() > 1 { chunk[1] as u16 } else { 0 };
                config_words.push(lo | (hi << 8));
            }
            asm::to_hex_regions(&[
                (0, program_words),
                (device.config.base_byte_addr, &config_words),
            ])
        }
        (Core::PicBaseline, Some(cb)) => {
            // Baseline config word is 12-bit (erased 0x0FFF), emitted as a
            // separate region at the config base like PIC18/PIC14E.
            let mut config_words = Vec::new();
            for chunk in cb.chunks(2) {
                let lo = chunk[0] as u16;
                let hi = if chunk.len() > 1 { chunk[1] as u16 } else { 0 };
                config_words.push(lo | (hi << 8));
            }
            asm::to_hex_regions(&[
                (0, program_words),
                (device.config.base_byte_addr, &config_words),
            ])
        }
        _ => asm::to_hex(program_words),
    }
}
