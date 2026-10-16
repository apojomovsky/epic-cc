# Compiler reference

This section is for you if you want to know exactly what the `epic-cc` compiler accepts and produces. It is reference material: for a first build, start with the [command line getting started guide](https://apojomovsky.github.io/epic-docs/getting-started/cli/).

`epic-cc` compiles C for 8-bit PIC microcontrollers in one step, from your sources to a hex file. Its front end is clang, and every later stage is part of the compiler itself.

## What is here

- The [CLI flags](cli-flags.md) page lists every option the command accepts.
- The [device matrix](https://apojomovsky.github.io/epic-docs/devices/) lists the parts you can pass to `--target`, with their cores and support status.

## Source and issues

The compiler is open source under the MIT licence. Its source and issue tracker are in the [epic-cc repository](https://github.com/apojomovsky/epic-cc).
