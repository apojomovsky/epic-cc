# 06 — Environment, tooling, and how to read the references

Machine: **Ubuntu 26.04**, x86-64. Repo: `/home/alexis/projects/pic8_compiler`.

## Tooling — do not install anything system-wide

**All build and test dependencies come from the docker dev image.** See
[`09-build-environment.md`](09-build-environment.md) for the full picture; the short
version:

```bash
docker build --target dev -t epic-cc-dev .   # first build is slow (clang)
docker run --rm -it -v "$PWD:/workspace" -w /workspace epic-cc-dev bash
```

This provides pinned `clang` 20.1.8, `rustc`/`cargo` 1.97.1, `gpasm` 1.5.2, `cvise`,
`creduce`, `csmith`, and `pdftotext`. Do **not** `apt install` these on the host — a
host-installed version shadowing the pinned one is exactly the drift the image exists to
prevent.

Host-provided and used as-is: `git`, `gh`, `curl`, `docker`.

Two things are **not** yet packaged and need their own packages later: `gpsim` and
`yarpgen`. Both are deferred; see [`09-build-environment.md`](09-build-environment.md).

## The XC8 install

**XC8 is not in `epic-cc-dev`. It lives in `epic-hal-toolchain:local`, and that is where
you must run it.** The epic-cc dev image sets `PIC8_XC8_ROOT=/opt/microchip/xc8/v4.00`
but does not install anything at that path, so invoking `xc8-cc` there fails with
`command not found`. Do not `apt install` or run the Microchip installer on the host: it
is licence-gated and the image boundary is the point.

```bash
# Version 4.00, PIC18 DFP 1.7.171 already unpacked in this image.
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/workspace" -w /workspace \
  epic-hal-toolchain:local \
  xc8-cc -mdfp=/opt/microchip/xc8/v4.00/pic/packs/Microchip.PIC18Fxxxx_DFP/xc8 \
         -mcpu=18f4550 -O2 -c file.c -o file.p1
```

The three facts that cost time to rediscover:

1. **`-mdfp` is mandatory and must point at the pack's `xc8` subdirectory**, not the pack
   root and not `/opt/dfp`. XC8 v4.00 does not auto-discover its device files, and the
   wrong suffix fails with `error: (2104) no device-support files found`.
2. **The image is `epic-hal-toolchain:local`**, built from `epic-hal`'s
   `docker/ci-toolchain/Dockerfile` (which installs XC8 plus the DFPs from
   `packs.download.microchip.com`). Its Makefile has the working invocation in
   `build-cmp/*/build.sh`. If the local tag is stale it may lack the packs; `docker images`
   shows what is actually present.
3. **Nothing in epic-cc reads `PIC8_XC8_ROOT` yet.** The XC8 differential described in
   [`05-verification.md`](05-verification.md) is not built: `docs/13` defers it to phase 6,
   and epic-hal's `build-cmp` scripts are the only XC8 invocations in the tree.

The install layout, for reference (paths inside the epic-hal image):

```
/opt/microchip/xc8/v4.00/
├── bin/              xc8-cc, xc8-ar, pic-objdump, pic-objcopy, deviceSupport.xml, …
├── pic/bin/          aspic aspic18 cgpic cgpic18 clang clist cromwell driver
│                     driver18 dump hexmate hlink libr
├── pic/packs/        Microchip.PIC16Fxxx_DFP, Microchip.PIC18Fxxxx_DFP,
│                     Microchip.PIC12-16F1xxx_DFP (each with an `xc8/` subdir)
├── pic-as/
├── avr/
└── docs/             MPLAB_XC8_C_Compiler_License.rtf, LLVM_LICENSE.txt, …
```

Verified working for PIC18: the recipe above compiles, links (`-ginhx32`), and prints the
`18F4550 Memory Summary` that the size references quote.

### Two critical facts about this install

1. **`pic/bin/clang` is NOT a general-purpose clang.** It is XC8's private front end and
   emits p-code consumed by `cgpic` — not usable LLVM IR. **Install a stock clang.**
2. **XC8 does not use LLVM for mid-range PIC.** `clang` is a front end only; the actual
   code generator for PIC14 is **`cgpic`**, the HI-TECH C lineage backend. The widespread
   belief that "XC8 is clang-based, therefore its codegen is LLVM" is wrong for our target.

### How XC8 may and may not be used

**Allowed:** invoke `xc8-cc` on source files and observe its output. That is our black-box
oracle ([`05-verification.md`](05-verification.md)).

**Forbidden:** disassembling or reverse-engineering any XC8 binary. See
[ADR-006](03-decisions.md).

---

## Reading the reference PDFs

The two books live in **`vendor/books/`** (gitignored — see
[`../vendor/README.md`](../vendor/README.md)):

```
vendor/books/muchnick-advanced-compiler-design-1997.pdf          887 pp
vendor/books/fraser-hanson-retargetable-c-compiler-lcc-1995.pdf  578 pp
```

> **These are gitignored deliberately.** They are copyrighted; never commit them, and never
> move them somewhere tracked.

`pdftotext` and `pdfinfo` come from the dev image, so run these inside the container.

### Method 1 — the `Read` tool (best for figures, tables, diagrams)

The `Read` tool renders PDF pages visually. Use the `pages` parameter, **max 20 pages per
call**:

```
Read(file_path="<repo>/vendor/books/muchnick-advanced-compiler-design-1997.pdf",
     pages="380-395")
```

Use this when the content is a figure, an algorithm listing with meaningful layout, or a
table.

### Method 2 — `pdftotext` + grep (best for searching)

```bash
pdfinfo "$BOOK"                       # page count, producer
pdftotext -f 50 -l 60 "$BOOK" -       # extract a page range to stdout
pdftotext "$BOOK" /tmp/book.txt       # extract everything
```

### ⚠️ The Muchnick gotcha — read this before searching it

**Muchnick's OCR text layer is encoded in fullwidth Unicode forms.** The text contains
`Ｃｈａｐｔｅｒ`, not `Chapter`. Plain ASCII grep **silently returns nothing** and you will
wrongly conclude the PDF has no text layer. Normalize first:

```bash
pdftotext vendor/books/muchnick-advanced-compiler-design-1997.pdf /tmp/muchnick.txt
python3 -c "
import unicodedata
t = open('/tmp/muchnick.txt', encoding='utf-8', errors='replace').read()
open('/tmp/muchnick_norm.txt','w',encoding='utf-8').write(unicodedata.normalize('NFKC', t))
"
grep -nE '^Chapter [0-9]+\.' /tmp/muchnick_norm.txt
```

The lcc PDF uses an Acrobat Paper Capture OCR layer and greps fine as-is.

### Verified chapter map — Muchnick

Line numbers are into `/tmp/muchnick_norm.txt` produced by the recipe above.

| Ch | Title | Why we care |
|---|---|---|
| 3 | Symbol-Table Structure | storage binding across banks |
| 4 | Intermediate Representations | our IR design |
| 6 | Producing Code Generators Automatically | instruction selection |
| 7 | Control-Flow Analysis | CFG construction |
| **8** | **Data-Flow Analysis** | **BANKSEL/PAGESEL placement** |
| **13** | **Redundancy Elimination** | **BANKSEL minimisation is PRE-shaped** |
| 15 | Procedure Optimizations | inlining, tail calls |
| **16** | **Register Allocation** | **overlay allocation is graph colouring** |
| 17 | Code Scheduling | peephole/scheduling |
| 18 | Control-Flow and Low-Level Optimizations | branch/skip optimisation |
| **19** | **Interprocedural Analysis and Optimization** | **whole-program call graph, the core of OCG** |

### Verified chapter map — lcc (Fraser & Hanson)

Relevant sections, from the book's own contents: *Code Generation Interface* (Interface
Records p.79, Interface Flags p.87, Interface Binding p.96) · *Structuring the Code
Generator* (§13.1, §13.2 p.354) · *Driving Code Generation* (§12.7 p.337) · *Register
Targeting* (p.397) · *Tracking the Register State* · *Allocating Registers* (p.413) ·
*Selecting Instructions* (pp.435, 503) · *Coordinating Instruction Selection* ·
*Selecting and Emitting Instructions* · *Code Generation and Optimization* (p.531)

The book presents **complete burg-style tree-pattern code generators** for MIPS R3000,
SPARC, and X86 as working source. That is the technique we want for PIC14 isel.

---

## Web research gotchas

- **`llvm-mos.org` is Cloudflare-protected.** `WebFetch` returns HTTP 403 and `curl` gets a
  JS challenge page. Use the **EuroLLVM 2022 slides PDF** instead, which is on `llvm.org`
  and fetches fine:
  `https://llvm.org/devmtg/2022-05/slides/2022EuroLLVM-LLVM-MOS-6502Backend.pdf`
  Fetch it, then read it with the `Read` tool's `pages` parameter — it is image-heavy, so
  text extraction alone returns nothing useful.

- **The llvm-pic repo is archived but its wiki is a separate, clonable git repo:**
  ```bash
  git clone --depth 1 https://github.com/llvm-pic/llvm-pic.wiki.git
  ```
  The main repo is a full LLVM fork (~500k commits) — do **not** clone it casually. Use the
  GitHub API to inspect it instead:
  ```bash
  gh api repos/llvm-pic/llvm-pic/contents/llvm/lib/Target/PICMid?ref=develop --jq '.[].name'
  ```

## Scratchpad

Use the session scratchpad for temporary files, never `/tmp` directly and never the repo:
`/tmp/claude-1000/-home-alexis-projects-pic8-compiler/<session-id>/scratchpad`
