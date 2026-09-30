# 40: Embedded C++ subset design (PIC18 only)

> **Status:** Phase 1 design for epic-cc#456. Feasibility probes were run
> against the pinned clang++ 20.1.8 in the docker dev image on current
> `origin/master`; every entry in §2 names the measured IR shape and the
> measured pipeline behavior, not a prediction. No compiler code changes
> land in this phase.

## 0. Frontend contract

`.cpp` inputs go through `clang++` (the same pinned 20.1.8 beside
`PIC8_CLANG_UNWRAPPED`) with the driver's existing flags plus two more:

```
-target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc -g
-ffp-contract=off -fpack-struct -fno-exceptions -fno-rtti
```

`-O1` stays fixed: alias and devirtualization observations in §2 depend
on it, and the driver never exposes an opt-out today (`BASE_ARGS` in
`crates/driver/src/clang.rs`). Two subset rules are enforced by clang
itself, verified by probe:

* `try/catch` under `-fno-exceptions` is a hard frontend error:
  `cannot use 'try' with exceptions disabled`.
* `dynamic_cast` under `-fno-rtti` is a hard frontend error:
  `use of dynamic_cast requires -frtti`.

No driver flag toggles either rule; they are always on for the `.cpp`
path (P0, #457).

## 1. Supported subset and rejections

Supported (all frontend-free, verified to reach our IR intact or with
the small extensions in §3):

* Classes with methods, member access, `this` (plain first `ptr`
  parameter, no backend work), access specifiers, `const` member
  functions, default arguments, overloading (resolved by clang).
* Constructors and destructors, including non-trivial ones, for
  automatic, static, and function-local-static duration.
* RAII on every exit path: clang emits the cleanup itself (single
  cleanup join at `-O0`, folded selects at `-O1`), so there is nothing
  to insert, only to preserve (§4, rescoped #459).
* Single-inheritance virtual functions: vtable const + double load +
  indirect call (§3 P3).
* Class and function templates (instantiated by clang; only the used
  specializations reach IR, verified: a two-type `Box`/`add2` program
  folds to one `main`).
* Namespaces (mangling only, verified: no IR impact beyond names).
* Operator overloading (mangling only, verified).
* C++ `struct` records (clang emits `%struct.Pair`, identical to C;
  verified sharing one global initializer shape with C).
* `extern "C"` declarations (unmangled, `external` linkage; verified:
  C/C++ TU mixing works at IR level).

Rejected, each with a clear error, never silent wrong code:

| Feature | Where it fails | Error |
|---|---|---|
| Exceptions (`try`/`throw`/`catch`) | clang++ | Frontend error, free |
| RTTI (`dynamic_cast`, `typeid`) | clang++ | Frontend error, free |
| Multiple inheritance | irparse (new check) | Explicit `multiple inheritance is not in the subset` panic on `_ZThn` thunks or a second vptr store. MI shape was not probed in Phase 1; the irparse ticket (#799) probes it first and keys the check on what it finds |
| Heap `new`/`delete` (`@_Znwj`, `@_Zdlvj`) | irparse (new check) | Explicit `heap new/delete are not in the subset` panic. The PIC18 arena heap (ADR-033) exists but EC++ bans the heap, so the ban is a subset rule, not a missing backend |
| Non-trivial static-duration destructor | accepted, not rejected | `__cxa_atexit` calls are erased by the P1 pass (see below), never silently kept |
| `thread_local`, placement details outside EC++ | clang/irparse | Existing panics apply |

`__cxa_atexit` erasure is semantically correct here, not a shortcut:
firmware never exits, and reset clears zero RAM and reruns constructors
from scratch (ADR-039), so a previous incarnation's destructors must
not run. Erasing is what the target's reset model requires; the pass
documents it at the call site it removes.

Function-local statics are supported via guard trivialization
(`__cxa_guard_acquire` always takes the init path once, lowered to a
plain `if (!guard)` on existing `br`/`select`). Limitation, enforced by
panic in P1: a function-local static must not be first-touched from ISR
context, where the init-once check cannot be trusted. The callgraph
already knows ISR reachability, so the check is cheap.

## 2. Measured breakage table (Phase 1 probes)

Corpus: class, RAII multi-return, single-inheritance virtuals (kept and
devirtualized shapes), class/function template, namespace + static
object, operator overload, non-trivial global ctor, function-local
static, heap `new`, `try`, `dynamic_cast`, C++ `struct` + `extern "C"`
mix, and a two-TU `inline` comdat pair. Compiled with the §0 flags,
parsed with `irparse::parse_ll`, merged with `wholeprog::merge`,
legalized, and built into a callgraph where reachable.

| # | Construct, IR shape | Measured behavior | Owner |
|---|---|---|---|
| 1 | `int main()` emits `@_Z4mainv` | `wholeprog::check_entry` counts `f.name == "main"` only, so every C++ TU fails entry (`expected exactly one main, found 0`, measured through the full probe) | P0 #457: entry accepts the single `_Z4mainv` as `main` |
| 2 | Classes emit `%class.X`, namespaced classes `%"class.ns::Y"` | `build_struct_table` collects `%struct.`/`%union.` only (`lib.rs:1151`), panic `unknown struct type %class.Reg` (measured on three probes) | New #799: accept the `class.` prefix and quoted names |
| 3 | `linkonce_odr` + `$sym = comdat any` (O0 inline/template) | Parses (lines skipped); `llvm-link` dedups across TUs (measured: two TUs merge to one `@_Z5twiceh`, callgraph edges exact) | None: ADR-011 holds unchanged; existing `sanitize_symbols` covers renames |
| 4 | `@llvm.global_ctors` with entries + `@_GLOBAL__sub_I_*` + `__cxa_atexit`/`@__dso_handle` | Non-empty `global_ctors` is silently dropped; the `__cxa_atexit` call then fails loud: `wholeprog: undefined symbols: __cxa_atexit` (measured) | P1 #458: startup pass calls ctors before `main`, erases `atexit`, trivializes guards |
| 5 | C1/D1 `alias` to C2/D2 | Alias lines dropped; `-O1` calls target C2/D2 directly (measured OK), `-O0` calls the alias (would fail entry resolution) | #799: resolve aliases to targets in irparse, no reliance on the optimizer |
| 6 | `dereferenceable(N)`, `initializes(...)` on definition params | `parse_param` panics: `unsupported param type token "dereferenceable(1)"` (measured; the call-arg path already tolerates both) | #799: accept both in `parse_param` |
| 7 | Vtable `@_ZTV4Base = constant { [3 x ptr] } { [3 x ptr] [ptr null, ptr null, ptr @tick] }` | `parse_array_elements` panics: `SPIKE LIMIT: array global initializer element "ptr @..."` (measured O0 and O1; the C-known ADR-022 gap) | P3 #460: decode fn-address elements, flash them via TBLRD, feed the address-taken set |
| 8 | Virtual dispatch: two loads + `tail call %fptr` | Parses; with an empty address-taken set legalize leaves `callees` empty (measured: callgraph has no edge, backend would trap) | P3 #460: P3 decode fills the set; the ADR-022 compare-and-call chain is the lowering, unchanged |
| 9 | `__cxa_guard_acquire/release` + `@_ZGV...` | Blocked behind #2 in probes; calls would be unresolved externs after it | P1 #458: trivialize as above |
| 10 | `new` emits `@_Znwj` declare + call (`allocsize`, `noalias`, `!heapallocsite` tolerated) | Parses; `wholeprog` rejects the undefined symbol (measured shape, loud but generic) | #799: name the subset rule in the panic |
| 11 | `store/load volatile` (memory-mapped register pattern) | `strip_attrs` drops `volatile` (`lib.rs:38`); `ir` carries no volatile bit (verified: no matches in `ir/src`) | #799: thread a volatile flag to isel and audit coalescing passes |
| 12 | `tail`, `mustprogress`/`nofree`/`nosync`/`willreturn`/`memory()`, `nobuiltin`, `readnone`/`nocapture`, `#dbg_value`, `poison`, `undef`, `!tbaa` with C++ TBAA names | All tolerated or correctly folded (measured: virtual, template, operator probes parse) | None |

Mangled names are gpasm-safe (measured: only `[A-Za-z0-9_]`);
`sanitize_symbols` needs no change for C++ beyond what ADR-011 covers.

## 3. Pipeline changes per stage and ordering

* P0 driver (#457): recognize `.cpp`, invoke `clang++` with §0 flags,
  keep the unconditional `llvm-link` merge (§2 row 3 says it holds for
  comdat), map the single `_Z4mainv` entry. `wholeprog` entry check
  accepts it; nothing downstream learns C++ exists.
* P0b irparse (#799, new, `area:frontend`): rows 2, 5, 6, 10, 11, plus
  the MI probe-then-check. One crate, all text-level, no IR redesign
  except the volatile flag.
* P1 startup (#458, scope extended): walk `llvm.global_ctors`, emit
  before-`main` calls, erase `__cxa_atexit` (documented, §1), resolve
  `__cxx_global_var_init` wrappers, trivialize `__cxa_guard_*`, panic
  on ISR-first-touch of a function-local static. PIC18 only.
* P2 RAII (#459, rescoped, not deleted): no insertion pass, clang owns
  cleanup emission (§1, measured). Work is preservation (legalize,
  irparse switch-splicing must not strand cleanup calls) plus the
  multi-return RAII fixture in P4 proving it. PIC18 only.
* P3 virtuals (#460): decode vtable consts (row 7), emit them as
  ADR-010 flash tables read with per-byte TBLPTR setup, feed fn
  addresses to the address-taken set so legalize fills `callees`, reuse
  the ADR-022 chain (row 8). Single inheritance only; thunks rejected
  by #799. PIC18 only.
* P4 fixtures + oracle (#461): §5.

Order: P0 and P0b in parallel (driver vs irparse, disjoint areas),
then P1, P2-prove, P3 (each needs P0b parsing), then P4. All pool
`dispatch-only` until the #456 go/no-go.

## 4. Resolved and deferred

* Resolved by probe: templates, namespaces, operators, `this`,
  exceptions, RTTI, comdat merge, C/C++ TU mixing are frontend-free.
* Rescoped, not dropped: #459 (preserve, not insert).
* Deferred to implementers: MI IR keying (#799 probes first); volatile
  audit list; ISR-first-touch check placement.

## 5. Verification plan

XC8 has no C++ mode, so the C differential oracle does not transfer.
`g++`-on-host is not an execution oracle either: host ABI (vtable
layout, `int` width) differs from the `msp430`-target clang that feeds
our backend; it can compute hand-assert values, never judge our HEX.
The plan is a mix, all inside existing gates:

1. Hand-computed asserts in the `Pic18` sim, primary. Same shape as
   `struct_wire_pic18_e2e`: driver binary to HEX for `p18f4550`, run,
   assert RAM/return values. Deterministic, no new infra.
2. C-twin differential, secondary. For virtual dispatch and RAII
   fixtures, write the equivalent C program (fn-pointer table, explicit
   cleanup), run both HEX in the sim, diff final RAM/`W`/`STATUS`.
   Reuses the proven C path as the comparator; catches vtable-layout
   and cleanup-path bugs hand asserts could miss.
3. `mdb` (MPLAB SIM through the epic-hal gate), independent executor.
   The vtable-dispatch and TBLRD const-read sequences land as replay
   specs in the existing harness (`crates/superopt`, `make mdb-oracle`
   tiers), not per-fixture runs. Until then those sequences are
   sim-only, stated in the fixture header.

Fixture set for #461: single inheritance + virtual dispatch (kept and
devirtualized), RAII across multiple returns, operator overload
(mangling guard), class template in two types, C++ `struct`/`extern "C"`
mix, negative fixtures (recursion ban and heap-`new` rejection still
fire on C++ input).

## 6. Go/no-go recommendation

Go. The work is five small, ordered, text-boundary changes reusing
solved machinery (ADR-010 flash, ADR-011 merge, ADR-022 dispatch,
ADR-039 reset); the only new IR is one volatile flag. Nothing in §2
suggests a backend redesign. Risk concentrates in P3 (const fn-table
decode feeding the address-taken set), which is bounded and
sim-testable.
