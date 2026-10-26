//! Backend interface and dispatch loop (ADR-0007).
//!
//! A backend (the hosted KVM backend, later the bare-metal VMX core) provides
//! a [`Vcpu`]. [`run_vcpu`] drives it until the guest stops, serving port I/O
//! through a [`PortIo`] handler, recording exits, and moving the VM's
//! [`VmLifecycle`] to `Stopped` or `Failed`.
//!
//! Port I/O is served *inside* [`Vcpu::run`], before the guest resumes:
//! hardware backends must hand the result of a port read back to the CPU
//! before the next VM entry. The exit is still returned, so every exit is
//! counted and traced.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::exit::{AccessSize, MmioAccess, VmExit};
use crate::lifecycle::{VmEvent, VmLifecycle, VmState};

/// Serves guest port I/O.
pub trait PortIo {
    /// A guest read of `size` bytes from `port`. Only the low `size` bytes of
    /// the result are used.
    fn read(&mut self, port: u16, size: AccessSize) -> u32;
    /// A guest write of `value` (already masked to `size`) to `port`.
    fn write(&mut self, port: u16, size: AccessSize, value: u32);
}

/// One virtual CPU of a backend.
pub trait Vcpu {
    /// Backend-specific failure to run the vCPU.
    type Error: fmt::Display;

    /// Run the guest until the next VM exit.
    ///
    /// Port I/O is served through `io` before this returns, and is still
    /// reported as [`VmExit::Io`].
    ///
    /// # Errors
    ///
    /// The backend could not run the vCPU, e.g. the host interrupted it or
    /// rejected the guest state.
    fn run(&mut self, io: &mut dyn PortIo) -> Result<VmExit, Self::Error>;
}

/// Bounds on one [`run_vcpu`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    /// Stop the VM (as `Failed`) after this many exits. Guards against exit
    /// storms (`research/security-analysis.md` §3.4).
    pub max_exits: u64,
    /// Keep at most this many exits in [`RunOutcome::trace`].
    pub trace_capacity: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_exits: 1_000_000,
            trace_capacity: 256,
        }
    }
}

/// Why [`run_vcpu`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The guest executed `HLT`. With no interrupt sources yet (M2), a halted
    /// guest can never resume, so this ends the run normally.
    Halted,
    /// The guest requested shutdown or triple-faulted.
    Shutdown,
    /// MMIO to a guest-physical address with neither RAM nor a device. No MMIO
    /// devices exist yet, so any MMIO exit ends up here.
    UnhandledMmio(MmioAccess),
    /// An exit the backend could not map to [`VmExit`].
    UnhandledExit {
        /// Backend-specific raw exit reason.
        raw_reason: u64,
    },
    /// [`RunLimits::max_exits`] reached.
    ExitLimit,
    /// The backend failed to run the vCPU.
    BackendError(String),
}

impl StopReason {
    /// Whether this ends the VM normally (`Stopped`) rather than as a failure.
    #[must_use]
    pub const fn is_normal(&self) -> bool {
        matches!(self, Self::Halted | Self::Shutdown)
    }
}

/// What happened during [`run_vcpu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Why the run ended.
    pub stop: StopReason,
    /// Total exits observed, including the final one.
    pub exits: u64,
    /// The first [`RunLimits::trace_capacity`] exits, in order.
    pub trace: Vec<VmExit>,
}

/// [`run_vcpu`] was called on a VM that is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotRunning(pub VmState);

impl fmt::Display for NotRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot run a vCPU of a VM in state {:?}", self.0)
    }
}

impl core::error::Error for NotRunning {}

/// Run `vcpu` until the guest stops, then move `lifecycle` to `Stopped`
/// (normal stop) or `Failed` (anything else).
///
/// # Errors
///
/// Returns [`NotRunning`] if `lifecycle` is not in [`VmState::Running`]; the
/// vCPU is not run.
pub fn run_vcpu<V: Vcpu>(
    vcpu: &mut V,
    io: &mut dyn PortIo,
    lifecycle: &mut VmLifecycle,
    limits: RunLimits,
) -> Result<RunOutcome, NotRunning> {
    if lifecycle.state() != VmState::Running {
        return Err(NotRunning(lifecycle.state()));
    }

    let mut exits: u64 = 0;
    let mut trace = Vec::new();
    let stop = loop {
        if exits >= limits.max_exits {
            break StopReason::ExitLimit;
        }
        let exit = match vcpu.run(io) {
            Ok(exit) => exit,
            Err(e) => break StopReason::BackendError(alloc::format!("{e}")),
        };
        exits = exits.saturating_add(1);
        if trace.len() < limits.trace_capacity {
            trace.push(exit);
        }
        match exit {
            VmExit::Io(_) | VmExit::Cpuid { .. } => {}
            VmExit::Hlt => break StopReason::Halted,
            VmExit::Shutdown => break StopReason::Shutdown,
            VmExit::Mmio(access) => break StopReason::UnhandledMmio(access),
            VmExit::Unhandled { raw_reason } => break StopReason::UnhandledExit { raw_reason },
        }
    };

    let event = if stop.is_normal() {
        VmEvent::Stop
    } else {
        VmEvent::Fail
    };
    // Running accepts both Stop and Fail, and the state was checked above.
    let _ = lifecycle.apply(event);
    Ok(RunOutcome { stop, exits, trace })
}
