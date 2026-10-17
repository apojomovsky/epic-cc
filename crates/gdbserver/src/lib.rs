//! The debugger adapter (epic-cc#259, extended to PIC18 in epic-cc#1312
//! and to PIC14E in epic-cc#1311):
//! a long-lived RSP server that drives a simulator control surface.
//! Loads the compiler's HEX (program words) and serves gdb over TCP;
//! the ELF+DWARF sidecar (`--sidecar` at compile time) stays on the
//! gdb side and is never read here.
//!
//! Register convention (docs/internals/design/34 §3): gdb sees an i386
//! architecture, so PIC state is mapped onto the x86 core registers:
//! `eip` <- PC (a word address on PIC14/PIC14E, a byte address on PIC18),
//! `eax` <- W, `ecx` <- STATUS, `edx` <- FSR/FSR0, `ebx` <- PCLATH/BSR.
//! Memory reads/writes address the RAM image physically below the
//! flash base and program flash as byte offsets above it.

use std::collections::HashSet;
use std::net::TcpListener;

use device::Core;
use gdbstub::common::Signal;
use gdbstub::conn::ConnectionExt;
use gdbstub::stub::run_blocking;
use gdbstub::stub::SingleThreadStopReason;
use gdbstub::stub::{DisconnectReason, GdbStub};
use gdbstub::target::ext::base::singlethread::SingleThreadBase;
use gdbstub::target::ext::base::singlethread::SingleThreadResume;
use gdbstub::target::ext::base::singlethread::SingleThreadSingleStep;
use gdbstub::target::Target;
use gdbstub::target::{TargetError, TargetResult};
use gdbstub_arch::x86::reg::X86CoreRegs;
use gdbstub_arch::x86::X86_SSE;
use pic14_sim::{Pic14, Pic14e, Pic18};

/// Parsed `epic-cc-gdbserver <hex> <sidecar.elf> --port <port>`
/// arguments. `--device` selects the core (a PIC14 part is the
/// default); only cores with a simulator control surface are accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub hex_path: String,
    pub sidecar_path: String,
    pub port: u16,
    pub device: Option<String>,
}

/// Parse the server command line. Errors are plain strings: `main`
/// prints the usage contract on failure.
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    const USAGE: &str =
        "usage: epic-cc-gdbserver <hex> <sidecar.elf> --port <port> [--device <name>]";
    let mut args = argv.iter();
    let hex_path = args.next().cloned().ok_or_else(|| USAGE.to_string())?;
    let sidecar_path = args.next().cloned().ok_or_else(|| USAGE.to_string())?;
    let mut port: Option<u16> = None;
    let mut device: Option<String> = None;
    while let Some(a) = args.next() {
        if let Some(p) = a.strip_prefix("--port=") {
            port = Some(p.parse().map_err(|_| "--port needs a number".to_string())?);
        } else if a == "--port" {
            let p = args
                .next()
                .ok_or_else(|| "--port needs a value".to_string())?;
            port = Some(p.parse().map_err(|_| "--port needs a number".to_string())?);
        } else if let Some(d) = a.strip_prefix("--device=") {
            device = Some(d.to_string());
        } else if a == "--device" {
            let d = args
                .next()
                .ok_or_else(|| "--device needs a value".to_string())?;
            device = Some(d.clone());
        } else {
            return Err(format!("epic-cc-gdbserver: unknown option {a}\n\n{USAGE}"));
        }
    }
    Ok(Args {
        hex_path,
        sidecar_path,
        port: port.ok_or_else(|| "missing --port".to_string())?,
        device,
    })
}

/// The simulator core `--device` names. Absent means PIC14, the
/// original single-core behavior; any other spelling resolves through
/// the device registry, and cores without a control surface are refused.
pub fn core_of(device: Option<&str>) -> Result<Core, String> {
    let Some(name) = device else {
        return Ok(Core::Pic14);
    };
    let dev =
        device::resolve(name).ok_or_else(|| format!("epic-cc-gdbserver: unknown device {name}"))?;
    match dev.core {
        Core::Pic14 | Core::Pic14e | Core::Pic18 => Ok(dev.core),
        other => Err(format!(
            "epic-cc-gdbserver: no simulator target for {other:?} (device {name})"
        )),
    }
}

/// Load the HEX program, validating the sidecar path the way `main`
/// does (the sidecar itself stays on the gdb side, unread here).
pub fn load_program(hex_path: &str, sidecar_path: &str, core: Core) -> Result<Vec<u16>, String> {
    let hex = std::fs::read_to_string(hex_path).map_err(|e| format!("read {hex_path}: {e}"))?;
    std::fs::metadata(sidecar_path).map_err(|e| format!("sidecar {sidecar_path}: {e}"))?;
    Ok(match core {
        Core::Pic14 => pic14_sim::parse_hex(&hex),
        Core::Pic14e => pic14_sim::parse_hex_pic14e(&hex),
        Core::Pic18 => pic14_sim::parse_hex_pic18(&hex),
        other => {
            return Err(format!(
                "epic-cc-gdbserver: no simulator target for {other:?}"
            ))
        }
    })
}

/// Serve one gdb session for `prog` over an already-accepted `stream`.
pub fn serve_stream(stream: std::net::TcpStream, prog: Vec<u16>, core: Core) {
    let connection: Box<dyn ConnectionExt<Error = std::io::Error>> = Box::new(stream);
    // `GdbStub` is typed over its target, so each core builds its own.
    let outcome = match core {
        Core::Pic14 => {
            let mut target = Pic14Target {
                sim: Pic14::new(prog),
                breakpoints: HashSet::new(),
                stepped: false,
            };
            GdbStub::new(connection).run_blocking::<EventLoop<Pic14Target>>(&mut target)
        }
        Core::Pic14e => {
            let mut target = Pic14eTarget {
                sim: Pic14e::new(prog),
                breakpoints: HashSet::new(),
                stepped: false,
            };
            GdbStub::new(connection).run_blocking::<EventLoop<Pic14eTarget>>(&mut target)
        }
        Core::Pic18 => {
            let mut target = Pic18Target {
                sim: Pic18::new(prog),
                breakpoints: HashSet::new(),
                stepped: false,
            };
            GdbStub::new(connection).run_blocking::<EventLoop<Pic18Target>>(&mut target)
        }
        other => {
            eprintln!("epic-cc-gdbserver: no simulator target for {other:?}");
            return;
        }
    };
    match outcome {
        Ok(DisconnectReason::Disconnect) | Ok(DisconnectReason::Kill) => {
            eprintln!("epic-cc-gdbserver: session ended")
        }
        Ok(other) => eprintln!("epic-cc-gdbserver: disconnected: {other:?}"),
        Err(e) => eprintln!("epic-cc-gdbserver: fatal: {e}"),
    }
}

/// Serve one gdb session for `prog` on an ephemeral loopback port,
/// returning the bound address. The in-process entry the acceptance
/// test uses so it never guesses a free port.
pub fn serve_once(
    prog: Vec<u16>,
    core: Core,
) -> std::io::Result<(std::net::SocketAddr, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        serve_stream(stream, prog, core);
    });
    Ok((addr, handle))
}

const RAM_SIZE: u32 = 0x200;
const PIC18_RAM_SIZE: u32 = 0x1000;
const PIC14E_RAM_SIZE: u32 = 0x1000;

pub struct Pic14Target {
    sim: Pic14,
    breakpoints: HashSet<u32>,
    /// Set by `step()`: the instruction already advanced synchronously,
    /// so the event loop must report `DoneStep` instead of running.
    stepped: bool,
}

pub struct Pic18Target {
    sim: Pic18,
    breakpoints: HashSet<u32>,
    /// Same `step()`/`DoneStep` contract as `Pic14Target::stepped`.
    stepped: bool,
}

pub struct Pic14eTarget {
    sim: Pic14e,
    breakpoints: HashSet<u32>,
    /// Same `step()`/`DoneStep` contract as `Pic14Target::stepped`.
    stepped: bool,
}

impl Pic14Target {
    fn read_mem_byte(&self, addr: u32) -> u8 {
        if addr < RAM_SIZE {
            self.sim.ram()[addr as usize]
        } else {
            // Flash byte space: word `w` lives at bytes `2w` (low) and
            // `2w + 1` (high), little-endian.
            let off = (addr - RAM_SIZE) as usize;
            let word = self.sim.read_prog_word((off / 2) as u16).unwrap_or(0);
            if off % 2 == 0 {
                (word & 0xFF) as u8
            } else {
                (word >> 8) as u8
            }
        }
    }
}

impl Pic18Target {
    fn read_mem_byte(&self, addr: u32) -> u8 {
        if addr < PIC18_RAM_SIZE {
            self.sim.ram()[addr as usize]
        } else {
            // Same flash byte space as PIC14, above the 4 KiB RAM.
            let off = (addr - PIC18_RAM_SIZE) as usize;
            let word = self.sim.read_prog_word((off / 2) as u16).unwrap_or(0);
            if off % 2 == 0 {
                (word & 0xFF) as u8
            } else {
                (word >> 8) as u8
            }
        }
    }
}

impl Pic14eTarget {
    fn read_mem_byte(&self, addr: u32) -> u8 {
        if addr < PIC14E_RAM_SIZE {
            self.sim.ram()[addr as usize]
        } else {
            // Same flash byte space as PIC18, above the 4 KiB RAM.
            let off = (addr - PIC14E_RAM_SIZE) as usize;
            let word = self.sim.read_prog_word((off / 2) as u16).unwrap_or(0);
            if off % 2 == 0 {
                (word & 0xFF) as u8
            } else {
                (word >> 8) as u8
            }
        }
    }
}

/// The core operations the RSP event loop drives. All targets share
/// the i386 register file and the breakpoint-watch run loop; only the
/// simulator underneath differs.
trait AdapterTarget: Target<Arch = X86_SSE, Error = &'static str> {
    fn halted(&self) -> bool;
    fn step_one(&mut self);
    fn pc_addr(&self) -> u32;
    fn at_breakpoint(&self) -> bool;
    fn take_stepped(&mut self) -> bool;

    /// Run at most `budget` instructions, reporting a stop reason when
    /// one lands, or `None` when the budget expired (the event loop
    /// polls the connection between budgets).
    fn run_to_stop_bounded(&mut self, budget: usize) -> Option<SingleThreadStopReason<u32>> {
        for _ in 0..budget {
            if self.halted() {
                return Some(SingleThreadStopReason::Terminated(Signal::SIGKILL));
            }
            // No pre-step breakpoint check: resuming on a breakpoint
            // address must advance (gdb lifts the stop's breakpoint
            // before continuing), and the post-step check below
            // catches every arrival including a self-loop.
            self.step_one();
            if self.halted() {
                return Some(SingleThreadStopReason::Terminated(Signal::SIGKILL));
            }
            if self.at_breakpoint() {
                return Some(SingleThreadStopReason::SwBreak(()));
            }
        }
        None
    }
}

impl AdapterTarget for Pic14Target {
    fn halted(&self) -> bool {
        self.sim.halted()
    }
    fn step_one(&mut self) {
        self.sim.step();
    }
    fn pc_addr(&self) -> u32 {
        self.sim.pc() as u32
    }
    fn at_breakpoint(&self) -> bool {
        self.breakpoints.contains(&self.pc_addr())
    }
    fn take_stepped(&mut self) -> bool {
        std::mem::replace(&mut self.stepped, false)
    }
}

impl AdapterTarget for Pic18Target {
    fn halted(&self) -> bool {
        self.sim.halted()
    }
    fn step_one(&mut self) {
        self.sim.step();
    }
    fn pc_addr(&self) -> u32 {
        self.sim.pc()
    }
    fn at_breakpoint(&self) -> bool {
        self.breakpoints.contains(&self.pc_addr())
    }
    fn take_stepped(&mut self) -> bool {
        std::mem::replace(&mut self.stepped, false)
    }
}

impl AdapterTarget for Pic14eTarget {
    fn halted(&self) -> bool {
        self.sim.halted()
    }
    fn step_one(&mut self) {
        self.sim.step();
    }
    fn pc_addr(&self) -> u32 {
        self.sim.pc() as u32
    }
    fn at_breakpoint(&self) -> bool {
        self.breakpoints.contains(&self.pc_addr())
    }
    fn take_stepped(&mut self) -> bool {
        std::mem::replace(&mut self.stepped, false)
    }
}

impl Target for Pic14Target {
    type Arch = X86_SSE;
    type Error = &'static str;

    #[inline(always)]
    fn base_ops(&mut self) -> gdbstub::target::ext::base::BaseOps<'_, Self::Arch, Self::Error> {
        gdbstub::target::ext::base::BaseOps::SingleThread(self)
    }

    // Breakpoints are implicit: `run_to_stop_bounded` watches the pc
    // set, no instruction words are patched, so the stub must not
    // refuse the session for lack of a real breakpoint backend.
    #[inline(always)]
    fn guard_rail_implicit_sw_breakpoints(&self) -> bool {
        true
    }

    #[inline(always)]
    fn support_breakpoints(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::BreakpointsOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::Breakpoints for Pic14Target {
    #[inline(always)]
    fn support_sw_breakpoint(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::SwBreakpointOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::SwBreakpoint for Pic14Target {
    fn add_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        self.breakpoints.insert(addr);
        Ok(true)
    }

    fn remove_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        Ok(self.breakpoints.remove(&addr))
    }
}

impl Target for Pic18Target {
    type Arch = X86_SSE;
    type Error = &'static str;

    #[inline(always)]
    fn base_ops(&mut self) -> gdbstub::target::ext::base::BaseOps<'_, Self::Arch, Self::Error> {
        gdbstub::target::ext::base::BaseOps::SingleThread(self)
    }

    // Same implicit-breakpoint contract as `Pic14Target`: the pc set
    // is watched, no flash words are patched.
    #[inline(always)]
    fn guard_rail_implicit_sw_breakpoints(&self) -> bool {
        true
    }

    #[inline(always)]
    fn support_breakpoints(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::BreakpointsOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::Breakpoints for Pic18Target {
    #[inline(always)]
    fn support_sw_breakpoint(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::SwBreakpointOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::SwBreakpoint for Pic18Target {
    fn add_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        self.breakpoints.insert(addr);
        Ok(true)
    }

    fn remove_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        Ok(self.breakpoints.remove(&addr))
    }
}

impl Target for Pic14eTarget {
    type Arch = X86_SSE;
    type Error = &'static str;

    #[inline(always)]
    fn base_ops(&mut self) -> gdbstub::target::ext::base::BaseOps<'_, Self::Arch, Self::Error> {
        gdbstub::target::ext::base::BaseOps::SingleThread(self)
    }

    // Same implicit-breakpoint contract as `Pic14Target`: the pc set
    // is watched, no flash words are patched.
    #[inline(always)]
    fn guard_rail_implicit_sw_breakpoints(&self) -> bool {
        true
    }

    #[inline(always)]
    fn support_breakpoints(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::BreakpointsOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::Breakpoints for Pic14eTarget {
    #[inline(always)]
    fn support_sw_breakpoint(
        &mut self,
    ) -> Option<gdbstub::target::ext::breakpoints::SwBreakpointOps<'_, Self>> {
        Some(self)
    }
}

impl gdbstub::target::ext::breakpoints::SwBreakpoint for Pic14eTarget {
    fn add_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        self.breakpoints.insert(addr);
        Ok(true)
    }

    fn remove_sw_breakpoint(
        &mut self,
        addr: u32,
        _kind: <Self::Arch as gdbstub::arch::Arch>::BreakpointKind,
    ) -> TargetResult<bool, Self> {
        Ok(self.breakpoints.remove(&addr))
    }
}

impl SingleThreadBase for Pic14Target {
    fn read_registers(&mut self, regs: &mut X86CoreRegs) -> TargetResult<(), Self> {
        regs.eax = self.sim.w() as u32;
        regs.ecx = self.sim.status() as u32;
        regs.edx = self.sim.fsr() as u32;
        regs.ebx = self.sim.pclath() as u32;
        regs.eip = self.sim.pc() as u32;
        Ok(())
    }

    fn write_registers(&mut self, regs: &X86CoreRegs) -> TargetResult<(), Self> {
        // Every mapped register reads back what gdb wrote: W, PC, the
        // full STATUS byte (bank bits included), FSR, and PCLATH.
        self.sim.set_w(regs.eax as u8);
        self.sim.set_pc(regs.eip as u16);
        self.sim.ram_mut()[0x03] = regs.ecx as u8;
        self.sim.ram_mut()[0x04] = regs.edx as u8;
        self.sim.ram_mut()[0x0A] = regs.ebx as u8;
        Ok(())
    }

    fn read_addrs(&mut self, start_addr: u32, data: &mut [u8]) -> TargetResult<usize, Self> {
        for (i, b) in data.iter_mut().enumerate() {
            *b = self.read_mem_byte(start_addr + i as u32);
        }
        Ok(data.len())
    }

    fn write_addrs(&mut self, start_addr: u32, data: &[u8]) -> TargetResult<(), Self> {
        let base = start_addr as usize;
        let end = base + data.len();
        // Flash is read-only through this surface; a write crossing
        // the RAM boundary fails instead of dropping its tail.
        if start_addr >= RAM_SIZE || end > RAM_SIZE as usize {
            return Err(TargetError::NonFatal);
        }
        self.sim.ram_mut()[base..end].copy_from_slice(data);
        Ok(())
    }

    fn support_resume(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadBase for Pic18Target {
    fn read_registers(&mut self, regs: &mut X86CoreRegs) -> TargetResult<(), Self> {
        regs.eax = self.sim.w() as u32;
        regs.ecx = self.sim.status() as u32;
        regs.edx = self.sim.fsr0() as u32;
        regs.ebx = self.sim.bsr() as u32;
        regs.eip = self.sim.pc();
        Ok(())
    }

    fn write_registers(&mut self, regs: &X86CoreRegs) -> TargetResult<(), Self> {
        // Same round-trip contract as PIC14: W, byte PC, STATUS, the
        // 12-bit FSR0 pair, and BSR.
        self.sim.set_w(regs.eax as u8);
        self.sim.set_pc(regs.eip);
        self.sim.ram_mut()[0xFD8] = regs.ecx as u8;
        self.sim.ram_mut()[0xFE9] = (regs.edx & 0xFF) as u8;
        self.sim.ram_mut()[0xFEA] = ((regs.edx >> 8) & 0xFF) as u8;
        self.sim.ram_mut()[0xFE0] = regs.ebx as u8;
        Ok(())
    }

    fn read_addrs(&mut self, start_addr: u32, data: &mut [u8]) -> TargetResult<usize, Self> {
        for (i, b) in data.iter_mut().enumerate() {
            *b = self.read_mem_byte(start_addr + i as u32);
        }
        Ok(data.len())
    }

    fn write_addrs(&mut self, start_addr: u32, data: &[u8]) -> TargetResult<(), Self> {
        let base = start_addr as usize;
        let end = base + data.len();
        // Flash is read-only through this surface; a write crossing
        // the RAM boundary fails instead of dropping its tail.
        if start_addr >= PIC18_RAM_SIZE || end > PIC18_RAM_SIZE as usize {
            return Err(TargetError::NonFatal);
        }
        self.sim.ram_mut()[base..end].copy_from_slice(data);
        Ok(())
    }

    fn support_resume(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadBase for Pic14eTarget {
    fn read_registers(&mut self, regs: &mut X86CoreRegs) -> TargetResult<(), Self> {
        regs.eax = self.sim.w() as u32;
        regs.ecx = self.sim.status() as u32;
        regs.edx = self.sim.fsr0() as u32;
        regs.ebx = self.sim.bsr() as u32;
        regs.eip = self.sim.pc() as u32;
        Ok(())
    }

    fn write_registers(&mut self, regs: &X86CoreRegs) -> TargetResult<(), Self> {
        // Same round-trip contract as PIC18, on PIC14E SFR addresses:
        // W, word PC, STATUS at 0x03, the FSR0 pair at 0x04/0x05,
        // and BSR at 0x08.
        self.sim.set_w(regs.eax as u8);
        self.sim.set_pc(regs.eip as u16);
        self.sim.ram_mut()[0x03] = regs.ecx as u8;
        self.sim.ram_mut()[0x04] = (regs.edx & 0xFF) as u8;
        self.sim.ram_mut()[0x05] = ((regs.edx >> 8) & 0xFF) as u8;
        self.sim.ram_mut()[0x08] = regs.ebx as u8;
        Ok(())
    }

    fn read_addrs(&mut self, start_addr: u32, data: &mut [u8]) -> TargetResult<usize, Self> {
        for (i, b) in data.iter_mut().enumerate() {
            *b = self.read_mem_byte(start_addr + i as u32);
        }
        Ok(data.len())
    }

    fn write_addrs(&mut self, start_addr: u32, data: &[u8]) -> TargetResult<(), Self> {
        let base = start_addr as usize;
        let end = base + data.len();
        // Flash is read-only through this surface; a write crossing
        // the RAM boundary fails instead of dropping its tail.
        if start_addr >= PIC14E_RAM_SIZE || end > PIC14E_RAM_SIZE as usize {
            return Err(TargetError::NonFatal);
        }
        self.sim.ram_mut()[base..end].copy_from_slice(data);
        Ok(())
    }

    fn support_resume(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadResumeOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadResume for Pic14Target {
    fn resume(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn support_single_step(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadResume for Pic18Target {
    fn resume(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn support_single_step(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadResume for Pic14eTarget {
    fn resume(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn support_single_step(
        &mut self,
    ) -> Option<gdbstub::target::ext::base::singlethread::SingleThreadSingleStepOps<'_, Self>> {
        Some(self)
    }
}

impl SingleThreadSingleStep for Pic14Target {
    fn step(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.sim.step();
        self.stepped = true;
        Ok(())
    }
}

impl SingleThreadSingleStep for Pic18Target {
    fn step(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.sim.step();
        self.stepped = true;
        Ok(())
    }
}

impl SingleThreadSingleStep for Pic14eTarget {
    fn step(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.sim.step();
        self.stepped = true;
        Ok(())
    }
}

struct EventLoop<T>(std::marker::PhantomData<T>);

impl<T: AdapterTarget> run_blocking::BlockingEventLoop for EventLoop<T> {
    type Target = T;
    type Connection = Box<dyn ConnectionExt<Error = std::io::Error>>;
    type StopReason = SingleThreadStopReason<u32>;

    fn wait_for_stop_reason(
        target: &mut T,
        conn: &mut Box<dyn ConnectionExt<Error = std::io::Error>>,
    ) -> Result<
        run_blocking::Event<SingleThreadStopReason<u32>>,
        run_blocking::WaitForStopReasonError<<T as Target>::Error, std::io::Error>,
    > {
        // A synchronous `step()` already advanced one instruction: report
        // it without running. Otherwise this loop runs the target while
        // polling the connection so an interrupt (^C) still lands.
        if target.take_stepped() {
            return Ok(run_blocking::Event::TargetStopped(
                SingleThreadStopReason::DoneStep,
            ));
        }
        let mut poll_incoming = || conn.peek().map(|b| b.is_some()).unwrap_or(true);
        loop {
            if poll_incoming() {
                let byte = conn
                    .read()
                    .map_err(run_blocking::WaitForStopReasonError::Connection)?;
                return Ok(run_blocking::Event::IncomingData(byte));
            }
            let stop = target.run_to_stop_bounded(64);
            match stop {
                Some(reason) => {
                    return Ok(run_blocking::Event::TargetStopped(reason));
                }
                None => continue,
            }
        }
    }

    fn on_interrupt(
        _target: &mut T,
    ) -> Result<Option<SingleThreadStopReason<u32>>, <T as Target>::Error> {
        Ok(Some(SingleThreadStopReason::Signal(Signal::SIGINT)))
    }
}
