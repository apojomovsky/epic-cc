# `second_oracle/`: device data for parts gputils does not know

gputils 1.5.x predates some parts (no `.lkr`, `gpasm -p` rejects the name),
so the gputils cross-check cannot run for them. Each file here covers one
such part: values transcribed from the DFP `.ini` (`ROMSIZE`, `RAMBANK`,
`COMMON`) and attested against the datasheet memory map cited in
`[provenance]`. The RAM gate uses a record only when gputils ships no
`.lkr` for the part; the flash gate only when `gpasm -p` rejects its name.
A part where the two halves disagree (`.lkr` without a `gpasm` entry)
is a broken install and fails loudly instead of taking either path.
The gate reports a record stale once gputils learns the part.

Schema (keys outside it are rejected; `common_ram`, `access_bank`,
`isr_w_shadow` and `isr_home_window` are optional and default to absent):

```toml
name = "p16f13145"
flash_words = 8192
ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]
common_ram = [0x0070, 0x007F]

[provenance]
pack = "Microchip.PIC16F1xxxx_DFP.1.29.444"
ini = "xc8/pic/dat/ini/16f13145.ini"
ini_sha256 = "<sha256 of that ini file>"
datasheet = "DS40002519D"
datasheet_section = "9.1 Table 9-1 (flash), 9.2 Figure 9-4 (RAM)"
```

`ram_banks` states the expected TOML values after the generator's carves
(bank 0 excludes the `common_ram` window, same convention as every shipped
TOML), not the raw ini `RAMBANK` line. The comparison is exact: a residual
is a defect in the TOML or the record, never a disclosure.
