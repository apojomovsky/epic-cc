# Extracting a device TOML from a datasheet

Use this only when a part has neither a Microchip ATDF nor a gputils `.lkr`.
Both of those are machine readable and are checked automatically; a datasheet
reading is not, which is why its output is a proposal a human must confirm.

## Before you start

Confirm the cheap sources really are absent:

```bash
ls "$PIC8_GPUTILS_SHARE"/lkr/<stem>_g.lkr        # stem has no leading p
python3 scripts/gen-device.py <part> --check
```

If either works, stop and use it. This path is strictly the fallback.

## Extract

`pdftotext` is in the dev image. Convert with layout preserved, or the tables
become unreadable:

```bash
pdftotext -layout <datasheet>.pdf -
```

Find and quote, verbatim, the table that gives each of:

| Field | Table to look for |
|---|---|
| `ram_banks`, `common_ram` | the register file map, one row per bank, GPR ranges only |
| `flash_words` | program memory organization, in **words** for PIC14 and PIC18 |
| `stack_depth` | the hardware stack description |
| `interrupt_vectors` | the interrupt vector address |
| `config` | the configuration word, with each field's mask, shift and values |

## Rules

1. **Never infer a boundary.** If a table gives GPR as `0x20-0x6F`, that is the
   range. Do not round it, extend it to a bank edge, or copy a neighbouring
   part's value.
2. **SFR ranges are not GPR.** Only generally usable RAM belongs in `ram_banks`.
3. **Mirrored common RAM appears once**, in `common_ram`, not repeated per bank.
4. **Record where each number came from.** Every value needs a table reference.
5. If the datasheet is ambiguous, say so and stop. An ambiguous value that gets
   guessed is exactly the failure this path exists to avoid.

## Output

Emit the TOML with this stanza, and nothing invented:

```toml
[provenance]
tier = "datasheet"
document = "DS<number><rev>"
tables = ["<the exact table captions you used>"]
ticket = "epic-cc#<the ticket tracking this device>"
```

Then hand it to a human with the quoted tables alongside, so the numbers can be
checked without reopening the PDF. Do not commit it yourself.
