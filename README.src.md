<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/epic-cc-logo-dark-mode.svg">
    <img src="docs/assets/epic-cc-logo-light-mode.svg" width="120" alt="Epic CC logo: a chip-temple inside a laurel wreath">
  </picture>
</p>

# epic-cc

A C compiler for 8-bit PIC microcontrollers, fully open source and MIT licensed, for hobbyists who want to write C and get a hex file.

[![CI](https://github.com/apojomovsky/epic-cc/actions/workflows/ci.yml/badge.svg)](https://github.com/apojomovsky/epic-cc/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Release](https://img.shields.io/github/v/release/apojomovsky/epic-cc)](https://github.com/apojomovsky/epic-cc/releases)

## Why epic-cc

- **Fully open source, MIT licensed.** After the clang front end, every stage that turns your C into a hex file lives in this repository, so you can read and audit it.
- **One command from C to hex file.** epic-cc compiles your whole program in one step, with no separate assembler or linker to install.
- **Checked against independent tools.** On a corpus of test programs, the output is compared byte for byte with the gpasm assembler, and by behavior with host clang and SDCC.

**Status:** Each part is marked supported, experimental, or planned in the [device reference](docs/reference/devices.md). See [What's in 1.0](https://apojomovsky.github.io/epic-docs/status/).

## The shortest example

Save this as `add.c` in the folder you get from the install step below:

```c
--8<-- "crates/driver/tests/fixtures/add.c"
```

Compile it for a PIC16F877A, a classic 8-bit PIC microcontroller:

```bash
--8<-- "docs/snippets/build-add.sh"
```

The program reads `in`, adds 1, and stores the result in `out`. `volatile` tells the compiler that something outside your code, such as hardware, can change these variables, so it must not optimize the accesses away.

When the command finishes without errors, it has written `add.hex`. A hex file (Intel hex format) is the file a programmer writes to the chip's flash memory, and the format a simulator loads.

If your code uses something the compiler does not support, it stops with an error that names the problem instead of writing a wrong hex file.

## Install

Download the release bundle for Linux with the [GitHub CLI](https://cli.github.com/) (`gh`), or download the zip from the [releases page](https://github.com/apojomovsky/epic-cc/releases/latest) in a browser. The bundle carries its own copy of clang, so you install nothing else:

```bash
--8<-- "docs/snippets/install.sh"
```

The folder you end up in holds the `epic-cc` program, its clang, and the definitions of the supported parts.

Check that it runs:

```bash
./epic-cc --help
```

On Windows, download the Windows zip from the same releases page. To build epic-cc from source, see [Contributing](CONTRIBUTING.md).

--8<-- "docs/snippets/ways-to-use-epic8.md"

To use epic-cc in a full project with ready-made hardware modules (UART, timers, and more) and Makefiles, start from [epic-hal](https://github.com/apojomovsky/epic-hal).

## Where next

- [Getting started](https://apojomovsky.github.io/epic-docs/getting-started/): your first blink, in the simulator and on a chip.
- [Guides](https://apojomovsky.github.io/epic-docs/guides/): how to do specific things.
- [Reference](docs/reference/devices.md): every part epic-cc accepts, and every [command-line flag](docs/reference/cli.md).
- [How it works](https://apojomovsky.github.io/epic-docs/explanation/): the design, for the curious.

## Contributing and licence

Contributions are welcome: see [Contributing](CONTRIBUTING.md). epic-cc is MIT licensed; see [LICENSE](LICENSE).
