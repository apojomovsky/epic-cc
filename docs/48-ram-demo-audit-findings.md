# 48: RAM audit across the HAL demos (epic-cc#838)

Audit, not a production commitment: decompose our own RAM footprint on
the four 18F4550 HAL sim demos into overlay, globals, and fixed
reserves, attribute each bucket to a mechanism in `crates/alloc`, and
price one implementation ticket per lever. Every count below is ours,
measured from the driver's `--map` and `--var-table` output on current
master. The comparison against a reference compiler that motivated the
audit lives in the private epic-benchmarks repo (ADR-006); no reference
numbers are quoted here.

Successor to docs/47 (#728), which audited the menu demo only at 755
bytes from a drifted fixture. The real menu build is 781 bytes (see
#835); this audit covers menu, control, pid, and bridge from epic-hal
HEAD's sim recipe.

## Method

Built with epic-cc 383bae0 from epic-hal 6784903's sim recipe
(`epic_build.py build --module <id> --mcu 18F4550 --variant sim
--toolchain epic-cc`), plus `--map` and `--var-table`. Reproduction is
that recipe with the two flags added. Frame sizes, full-interval object
bytes, and call-graph depth come from a throwaway instrumented dump of
`alloc::allocate` (per-function base, colored size, depth end, and
byval/sret/alloca/va bytes); the ablation prices below re-ran the same
builds with GEP operand propagation and loop live-in/out extension
gated off. The probes stayed out of the tree.

## Our footprint (18F4550 sim)

| demo | flash words | RAM bytes | bank0 | fixed | ISR region | overlay depth | globals |
|---|---|---|---|---|---|---|---|
| menu | 9634 | 781 | 765 | 16 | 14 | 229 (`0x010-0x0F5`) | 45 (528B) |
| control | 9962 | 977 | 961 | 16 | 14 | 208 (`0x010-0x0E0`) | 72 (741B) |
| pid | 3309 | 500 | 484 | 16 | 10 | 112 (`0x010-0x080`) | 49 (367B) |
| bridge | 14026 | 1312 | 1296 | 16 | 16 | 391 (`0x399-0x520`) | 46 (840B) |

Fixed is 4 bytes of retval/flag plus the 12-byte ISR save on every
demo; all four have an ISR. Menu, control, and pid place the overlay
frames-first below the globals. Bridge keeps globals-first:
pinned peripheral mirrors starting at `0x010` (`EPIC_PLACE` timer, SSP,
USART, and ADC handle storage the bridge demo owns) sit inside any
frames-first overlay span, so the fallback fires and the overlay starts
at `0x399`. Order moves no demand, only its position, so this costs
bridge nothing; it only explains the shape.

- menu (208): main 10/0, menu_demo_init 119/55, redraw 19/0,
  redraw_status 23/0, epic_lcd_print 10/0, gpio4_delay_us 9/0,
  __udiv_u32 18/10.
- control (187): main 10/0, epic_taskmgr_run 26/0,
  control_demo_task_console 13/0, console_rx_byte 86/0,
  epic_serial_put_i16 4/0, epic_serial_put_idec 30/0, __udiv_u32 18/10.
- pid (95): main 41/0, epic_pid_update 38/0, epic_math_mul_s16 16/0.
- bridge (368): main 97/13, epic_taskmgr_run 26/0,
  bridge_demo_task_bridge 159/72, staged_read 59/0,
  epic_serial_write 11/0, epic_dispatch_all_irqs 5/0,
  TIMER2_IRQHandler 3/0, epic_tick_on_overflow_isr 8/0. The ISR frames
  ride the dispatch all-IRQ path, which calls the timer handler through
  the shared-storage dispatch.

## Attribution and priced levers

Full-interval objects (byval/sret params, allocas, va regions take a
whole-function slot today) on the deepest path: menu 65B, control 10B,
pid 0B, bridge 85B. Whole-program totals are 129/118/32/180B. Filed as
the precise-live-ranges ticket; the numbers are upper bounds, since a
byval copy co-live with its caller needs an ABI design (#737), not only
coloring.

The remaining path width is simultaneous liveness under linear block
order, concentrated in a few named frames: console_rx_byte 86B,
staged_read 59B, bridge_demo_task_bridge 87B past its objects,
menu_demo_init 64B past its objects, control_demo_task_control 40B,
menu_demo_task_ui 27B. Loop live-in/out extension was ablated to 3B
menu, 7B control, 1B pid, 16B bridge: real but small, so the wide
frames are mostly genuine overlap, and the ticket attacks them by name.

Seven legalize-duplicated `_isr` frames in menu and control (32B of
copies), ten in bridge (46B), one in pid (3B). They stack on the ISR
path at 6+8 (menu, control) and 5+3+8 (bridge); folding them into one
shared dispatch frame shrinks the ISR region toward its widest copy,
about 6B on menu and control, 8B on bridge, 0B on pid.

Flag globals: 11/11/10/10 one-byte scalars per demo, all `unsigned
char`, no C `bool`. Packing every flag bit saves at most about 10B per
demo; the realistic set (dirty/done bits, not ring indices) is smaller.
Narrowing candidates are the 3/18/14/3 `unsigned short` scalars plus
peripheral mirrors, pending range proofs. One ticket covers both, priced
small on purpose.

Not levers, recorded so nobody re-prices them: GEP-base propagation
ablates to 0-1B per demo. Routine rounding leaves no gap anywhere (no
frame base sits past its callers' physical ends). The four-byte retval
region is owned by open ticket #738, which is RAM-neutral. The
const-to-RAM survivors (`.str` 2B menu, `.str.17` 2B control, two 15B
pid strings) are the set #779 pinned as required; the 12B `s_fmt_buf`
copies are writable format scratch, which is source demand.

## Recommendation (filed as tickets)

- Precise live ranges for byval, sret, alloca, and va slots
  (path bounds 65/10/0/85B, whole-program 129/118/32/180B).
- Shrink the named wide frames (86/59/87/64/40/27B upper bounds;
  loop extension measured at 1-16B inside those).
- Fold the `_isr` duplicates into the shared dispatch frame
  (about 6/6/0/8B off the ISR region).
- Pack flag bytes and narrow proven-range globals (at most about
  10B plus the u16 candidate list per demo).
- Attribute the remainder once the four land (sequenced after the
  first two).

## Residuals

- The `--map`, `--var-table`, and instrumented dumps used here were
  scratch and are deleted; reproduction is the recipe above.
- Sim-harness bytes (report line buffer, harness init frame, stimulus
  tasks) ship in every sim build and are counted above; the target
  firmware does not carry them.
- PIC14/PIC14E overlays are unpriced; their region shapes differ, so
  none of the above transfers without remeasuring.
- The small-program floor is owned by #837; findings are linked, not
  duplicated.
