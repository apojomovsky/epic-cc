# vendor/ — user-supplied material

Everything under this directory except this README is **gitignored**. It holds proprietary
installers, copyrighted documentation, and reference books that must not be committed.

Nothing here is required to build the compiler or run the core test suite. Items are
**optional capabilities**: when present they enable extra oracles and reference material;
when absent, tooling must degrade gracefully with a clear message rather than fail.

## Layout

```
vendor/
├── README.md              (this file — the only committed thing here)
├── microchip/
│   ├── installers/        XC8 / MPLAB installers (.run, .tar.gz)
│   ├── datasheets/        Microchip PDFs
│   └── device-data/       .inc / .pic / device description files
└── books/                 compiler reference PDFs
```

## What to put where

### `microchip/installers/`

Proprietary installers, kept so the environment can be rebuilt on another machine.

- `xc8-v4.00-full-install-linux-x64.run` — the XC8 compiler installer
- MPLAB IPE / MPLAB X installers, if you want flashing tooling reproducible

**Note:** XC8 does not need to be installed *from here* to be used. The toolchain looks for
an existing install via `$PIC8_XC8_ROOT` (default `/opt/microchip/xc8/v4.00`). These
installers are for reproducibility and disaster recovery.

### `microchip/datasheets/`

Free downloads from Microchip. These resolve the `[VERIFY]` items in
[`../docs/01-target-pic14.md`](../docs/01-target-pic14.md), which is currently the single
biggest source of unconfirmed facts in the design.

| File | Doc № | Why we need it |
|---|---|---|
| PIC16F87XA Data Sheet | DS39582 | 877A memory map, bank ranges, common RAM extent, flash size, config words |
| PICmicro Mid-Range MCU Family Reference Manual | DS33023 | Authoritative PIC14 core architecture and instruction semantics |
| MPLAB XC8 C Compiler User's Guide | DS52053 or later | Best *public* description of how a working PIC C compiler makes ABI, memory, and pointer-scoping decisions |
| MPASM Reference | DS33014 | Assembler directives and syntax to stay compatible with |

### `microchip/device-data/`

Device description inputs. `gputils` ships `.inc` files that are a good source for the
device database described in [ADR-004](../docs/03-decisions.md) — in the Nix shell they are
already available under `$(dirname $(readlink -f $PIC8_GPASM))/../share/gputils/header`, so
copying them here is only needed if you want a specific version frozen.

### `books/`

Compiler reference PDFs. Currently:

- `Advanced Compiler Design and Implementation` — Muchnick, 1997, 887 pp
- `A retargetable C compiler - design and implementation` — Fraser & Hanson (lcc), 1995, 578 pp

**Reading these:** see [`../docs/06-environment.md`](../docs/06-environment.md). In
particular, Muchnick's OCR text layer is fullwidth Unicode and needs NFKC normalisation
before `grep` will match anything.

## For agents

Check what is actually present before assuming — do not hard-code filenames, glob instead:

```bash
ls vendor/microchip/installers/ vendor/microchip/datasheets/ vendor/books/ 2>/dev/null
```

If something you need is missing, **say so specifically** ("DS39582 is not in
`vendor/microchip/datasheets/`; I need it to confirm the bank layout") rather than guessing
at the values or silently proceeding.
