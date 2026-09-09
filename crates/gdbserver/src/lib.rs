//! The debugger adapter (epic-cc#259): a long-lived RSP server
//! that drives the `Pic14` control surface. Loads the compiler's
//! HEX (program words) and serves gdb over TCP; the ELF+DWARF sidecar
//! (`--sidecar` at compile time) stays on the gdb side and is never read
//! here.
//!
//! Register convention (docs/34 §3): gdb sees an i386
//! architecture, so PIC14 state is mapped onto the x86 core registers:
//! `eip` <- PC (word address), `eax` <- W, `ecx` <- STATUS, `edx` <- FSR,
//! `ebx` <- PCLATH. Memory reads/writes address the RAM image physically
//! below 0x200 and program flash as byte offsets above it.

use std::collections::HashSet;
use std::net::TcpListener;

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
use pic14_sim::Pic14;

/// Parsed `epic-cc-gdbserver <hex> <sidecar.elf> --port <port>` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub hex_path: String,
    pub sidecar_path: String,
    pub port: u16,
}

/// Parse the server command line. Errors are plain strings: `main`
/// prints the usage contract on failure.
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    const USAGE: &str = "usage: epic-cc-gdbserver <hex> <sidecar.elf> --port <port>";
    let mut args = argv.iter();
    let hex_path = args.next().cloned().ok_or_else(|| USAGE.to_string())?;
    let sidecar_path = args.next().cloned().ok_or_else(|| USAGE.to_string())?;
    let mut port: Option<u16> = None;
    while let Some(a) = args.next() {
        if let Some(p) = a.strip_prefix("--port=") {
            port = Some(p.parse().map_err(|_| "--port needs a number".to_string())?);
        } else if a == "--port" {
            let p = args
                .next()
                .ok_or_else(|| "--port needs a value".to_string())?;
            port = Some(p.parse().map_err(|_| "--port needs a number".to_string())?);
        } else {
            return Err(format!("epic-cc-gdbserver: unknown option {a}\n\n{USAGE}"));
        }
    }
    Ok(Args {
        hex_path,
        sidecar_path,
        port: port.ok_or_else(|| "missing --port".to_string())?,
    })
}

/// Load the HEX program, validating the sidecar path the way `main`
/// does (the sidecar itself stays on the gdb side, unread here).
pub fn load_program(hex_path: &str, sidecar_path: &str) -> Result<Vec<u16>, String> {
    let hex = std::fs::read_to_string(hex_path).map_err(|e| format!("read {hex_path}: {e}"))?;
    std::fs::metadata(sidecar_path).map_err(|e| format!("sidecar {sidecar_path}: {e}"))?;
    Ok(pic14_sim::parse_hex(&hex))
}

/// Serve one gdb session for `prog` over an already-accepted `stream`.
pub fn serve_stream(stream: std::net::TcpStream, prog: Vec<u16>) {
    let mut target = Pic14Target {
        sim: Pic14::new(prog),
        breakpoints: HashSet::new(),
        stepped: false,
    };
    let connection: Box<dyn ConnectionExt<Error = std::io::Error>> = Box::new(stream);
    let gdb = GdbStub::new(connection);
    match gdb.run_blocking::<GdbEventLoop>(&mut target) {
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
) -> std::io::Result<(std::net::SocketAddr, std::thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let handle = std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        serve_stream(stream, prog);
    });
    Ok((addr, handle))
}

const RAM_SIZE: u32 = 0x200;

pub struct Pic14Target {
    sim: Pic14,
    breakpoints: HashSet<u32>,
    /// Set by `step()`: the instruction already advanced synchronously,
    /// so the event loop must report `DoneStep` instead of running.
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

impl SingleThreadSingleStep for Pic14Target {
    fn step(&mut self, _signal: Option<Signal>) -> Result<(), Self::Error> {
        self.sim.step();
        self.stepped = true;
        Ok(())
    }
}

enum GdbEventLoop {}

impl run_blocking::BlockingEventLoop for GdbEventLoop {
    type Target = Pic14Target;
    type Connection = Box<dyn ConnectionExt<Error = std::io::Error>>;
    type StopReason = SingleThreadStopReason<u32>;

    fn wait_for_stop_reason(
        target: &mut Pic14Target,
        conn: &mut Box<dyn ConnectionExt<Error = std::io::Error>>,
    ) -> Result<
        run_blocking::Event<SingleThreadStopReason<u32>>,
        run_blocking::WaitForStopReasonError<<Pic14Target as Target>::Error, std::io::Error>,
    > {
        // A synchronous `step()` already advanced one instruction: report
        // it without running. Otherwise this loop runs the target while
        // polling the connection so an interrupt (^C) still lands.
        if std::mem::replace(&mut target.stepped, false) {
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
        _target: &mut Pic14Target,
    ) -> Result<Option<SingleThreadStopReason<u32>>, &'static str> {
        Ok(Some(SingleThreadStopReason::Signal(Signal::SIGINT)))
    }
}

impl Pic14Target {
    /// Run at most `budget` instructions, reporting a stop reason when
    /// one lands, or `None` when the budget expired (the event loop
    /// polls the connection between budgets).
    fn run_to_stop_bounded(&mut self, budget: usize) -> Option<SingleThreadStopReason<u32>> {
        for _ in 0..budget {
            if self.sim.halted() {
                return Some(SingleThreadStopReason::Terminated(Signal::SIGKILL));
            }
            // No pre-step breakpoint check: resuming on a breakpoint
            // address must advance (gdb lifts the stop's breakpoint
            // before continuing), and the post-step check below
            // catches every arrival including a self-loop.
            self.sim.step();
            if self.sim.halted() {
                return Some(SingleThreadStopReason::Terminated(Signal::SIGKILL));
            }
            if self.breakpoints.contains(&(self.sim.pc() as u32)) {
                return Some(SingleThreadStopReason::SwBreak(()));
            }
        }
        None
    }
}
