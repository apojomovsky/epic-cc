# 49: RAM gap attribution per demo (epic-cc#857)

Audit, not a production commitment: attribute every byte of our own
RAM footprint on the four 18F4550 HAL sim demos, mark each bucket as
compiler overhead, source demand, or harness-only, and map the
overhead to lever tickets. Every count below is ours, measured from
the driver's `--map` and `--var-table` output plus a throwaway
instrumented dump of `alloc::allocate` (per-function base, colored
size, depth, and object-class bytes; the probes stayed out of the
tree). The comparison against a reference compiler that motivated
this work lives in the private epic-benchmarks repo (ADR-006); no
reference numbers are quoted here.

Successor to docs/48 (#838), which priced the four lever tickets
from upper bounds. This audit re-runs the recipe on current master
and accounts the full footprint, not just the lever deltas.

## Method

Built with epic-cc 46783db from epic-hal 75811d9's sim recipe
(`epic_build.py build --module <id> --mcu 18F4550 --variant sim
--toolchain epic-cc`), plus `--map` and `--var-table`.
Reproduction is that recipe with the two flags added. Defining-file
attribution for every global and function comes from the DWARF
`DIFile` nodes in the saved `merged_opt.ll`, joined to the map by
symbol name.

## Our footprint (18F4550 sim)

| demo | RAM | overlay | main path | ISR base gap | ISR region | globals span | holes | fixed |
|---|---|---|---|---|---|---|---|---|
| menu | 781 | `0x010-0x0F5` (229) | 208 | 7 | 14 | `0x0F6-0x30D` (535) | 7+1 | 16 |
| control | 977 | `0x010-0x0E0` (208) | 187 | 7 | 14 | `0x0E0-0x3D1` (753) | 12 | 16 |
| pid | 500 | `0x010-0x080` (112) | 95 | 7 | 10 | `0x080-0x1F4` (372) | 5 | 16 |
| bridge | 1312 | `0x399-0x520` (391) | 365 | 10 | 16 | `0x010-0x399` (905) | 65 | 16 |

Overlay is `main path + ISR base gap + ISR region` on all four: the
disjoint ISR region sits above the main context, so globals stack
above the ISR top. The gap holds the 7-byte carved ISR save
(`isr-save` in the map, epic-cc#477); bridge measures 10, with 3
unattributed bytes between its path top and the save. Holes are
intra-span alignment gaps; menu carries one more leading byte (globals
open at `0x0F6`, the first even address). Fixed is 4 bytes of
retval/flag plus the 12-byte ISR save on every demo. No demo has
startup frames: `main` is a call-graph root, there are no `__start`
or `__epic_config` functions, and zero-init is code, not RAM.

## Deepest paths by function

Sizes are colored frame bytes; the class columns are full-interval
object bytes inside the frame (the #853 set).

menu (208): main 10, menu_demo_init 119 (55 alloca), redraw 19,
redraw_status 23, epic_lcd_print 10, gpio4_delay_us 9,
`__udiv_u32` 18 (10 alloca, runtime routine). Init-once frames sit at
the same base but off the path and cost nothing: epic_harness_init
23 (18 alloca), epic_taskmgr_attach_timer0 18 (12 alloca).

control (187): main 10, epic_taskmgr_run 26,
control_demo_task_console 13, console_rx_byte 86,
epic_serial_put_i16 4, epic_serial_put_idec 30, `__udiv_u32` 18
(10 alloca). Off path: control_demo_init 58 (42 alloca),
control_demo_task_control 40, `__mul_u16` 18 (14 alloca).

pid (95): main 41 (the sim main itself, no alloca),
epic_pid_update 38, epic_math_mul_s16 16. Off path:
epic_harness_init 23 (18 alloca), USART_ComputeSPBRG 14.

bridge (365): main 97 (13 alloca), epic_taskmgr_run 26,
bridge_demo_task_bridge 159 (72 alloca), staged_read 59,
epic_serial_write 11, epic_dispatch_all_irqs 5,
USART_TX_IRQHandler 4, EPIC_IRQ_GetFlag 4. Off path:
bridge_demo_init 82 (20 alloca), handle_read_bits 36,
handle_read_regs 30, `__mul_u16`/`__udiv_u32` 18 each.

## ISR regions

menu/control: dispatch copy 6 plus the widest overlaid copy
(timer0 overflow 8); 5 more `_isr` copies ride inside the same 14
bytes. pid: dispatch 6 plus GetFlag 4 with one `_isr` copy inside
(10 total). bridge: dispatch copy 5, TX handler copy 4, serial TX
copy 4, disable-source copy 3, with 6 more `_isr` copies overlaid
(16 total). Folding the copies into one shared dispatch frame (#855)
saves about 6/6/0/8 bytes per demo.

## Globals by owning module

menu (45 globals, 528 bytes): serial 164, taskmgr 90, demo-core 85,
ccp 66, hal-core 27, harness sim-main 26, usart 20, adc 15, timer0
14, timer2 9, harness mdb 4, tick 4, lcd 4.

control (72 globals, 741 bytes): harness sim-main 166, serial 164,
demo-core 102, taskmgr 90, ccp 66, math 64, hal-core 27, usart 20,
adc 15, timer0 14, timer2 9, harness mdb 4.

pid (49 globals, 367 bytes): serial 164, math 64, harness sim-main
54, hal-core 27, usart 20, timer0 12, ssp 11, timer2 7, harness mdb
4, tick 4.

bridge (46 globals, 840 bytes): demo-core 259, serial 164, harness
sim-main 106, taskmgr 90, modbus 77, bus 38, hal-core 27, usart 20,
adc 15, timer0 14, ssp 13, timer2 9, harness mdb 4, tick 4.

Largest single demand items: the task table `g_tasks` (80 bytes,
pinned at `0x110` on every demo), the serial rings (32 TX plus 32
RX), the bridge demo struct (259), the control stimulus table
(150), the bridge stimulus frames (56 plus 20 plus 20), and the
peripheral handle storages (ccp 66, usart 18, adc 11 to 15, timers
7 to 14, ssp 11 to 13).

## Overhead, demand, harness-only

Compiler overhead with a lever: full-interval allocas on the path
(#853: menu 65, control 10, pid 0, bridge 85 upper bounds,
realizable single digits per the #853 prototype); the named wide
frames (#854); the `_isr` copies (#855); flag bytes and wide
scalars (#856: 10 to 11 one-byte flags and 3 to 18 short scalars
per demo); const-to-RAM copies (menu 6, control 2, pid 30,
bridge 0; pid's two 15-byte strings are harness report text, so
they belong to the harness row below); the serial static
workarounds below; alignment holes (5 to 12, bridge 65 around its
pin); the 7-byte carved save plus fixed 16.

Compiler-gated source workarounds, priced here for the first time:
serial carries 76 static bytes per demo that exist only under
`__EPIC_CC__` (the 64-byte put-string scratch plus the 12-byte
format buffer; the other toolchain uses stack buffers), the harness
report line adds 27 more (a RAM copy of a const marker, same
limitation), and the bus module carries 16 mutable bytes on bridge
(its I2C/SPI defaults are `static const` on the other path). All
are fallout from missing compiler features (const-pointer call
args, flash const structs), not allocator policy. Target about 103
per demo plus 16 on bridge.

Source demand: rings, tables, demo state, peripheral storages, and
math state, as listed above. The bridge pin (`g_tasks` at `0x110`)
forces globals-first there, which costs no demand bytes, only the
65 alignment holes around it.

Harness-only (a target build does not carry these): sim-main globals
(menu 26, control 166, pid 54, bridge 106) plus the mdb cycle
counter (4), and the sim main's own frame (10, 10, 41, 97) in place
of the target main's. The report line buffer (27) and the harness
init frame (23 with 18 alloca) stay in every build; init's frame is
off every depth path, so it costs no RAM today.

Not levers: init-once subtree sharing (init frames sit off path at
zero cost, confirming the #794 withdrawal); routine single-bank
rounding (no base moved for it on any demo); the retval region
(#738, RAM-neutral); dead-slot reservation (#862, about a byte per
small program, unmeasured on demos).

## Residuals

- The `--map`, `--var-table`, and instrumented dumps used here were
  scratch and are deleted; reproduction is the recipe above.
- Bridge's main-context path sums to 365 while the allocator's
  `depth_end(main)` reads 368: the 3-byte tail is ISR-context
  frames reachable from the main-context dispatch, which the ISR
  region places separately. The carved save therefore sits at
  `0x509`, 3 bytes above the main-context top (`0x506`); every byte
  is placed, none is slack.
- PIC14/PIC14E overlays are unpriced; their region shapes differ, so
  none of the above transfers without remeasuring.
- The ablation that locates the remaining cross-compiler gap by
  module (totals only, both toolchains) is recorded in
  epic-benchmarks; its lever tickets are filed from that table.
