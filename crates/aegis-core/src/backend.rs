//! Backend interface and dispatch loop (ADR-0007).
//!
//! A backend (the hosted KVM backend, later the bare-metal VMX core) provides
//! a [`Vcpu`]. [`run_vcpu`] drives one vCPU until the guest stops: it serves
//! port I/O and MMIO through a [`GuestIo`] handler, records exits, and moves
//! the VM's [`VmLifecycle`] to `Stopped` or `Failed`.
//!
//! For SMP guests, each vCPU runs [`run_vcpu_until`] on its own thread with a
//! shared [`StopFlag`]. The first vCPU to stop sets the flag. The others
//! notice it, either before their next entry or when the backend reports that
//! the host interrupted the vCPU, and stop with [`StopReason::Requested`].
//! [`vm_stop_event`] then turns all the stop reasons into one lifecycle event.
//!
//! Guest I/O is served *inside* [`Vcpu::run`], before the guest resumes:
//! hardware backends must hand a read's result back to the CPU before the next
//! VM entry. The exit is still returned, so every exit is counted and traced.

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

/// Serves guest MMIO to addresses that have no RAM behind them.
pub trait MmioIo {
    /// A guest read of `size` bytes at `gpa`, or `None` if no device is there.
    fn mmio_read(&mut self, gpa: u64, size: AccessSize) -> Option<u64>;
    /// A guest write of `value` (masked to `size`) at `gpa`. Returns `false`
    /// if no device is there.
    fn mmio_write(&mut self, gpa: u64, size: AccessSize, value: u64) -> bool;
}

/// Everything a vCPU needs served: port I/O and MMIO.
pub trait GuestIo: PortIo + MmioIo {}

impl<T: PortIo + MmioIo + ?Sized> GuestIo for T {}

/// One virtual CPU of a backend.
pub trait Vcpu {
    /// Backend-specific failure to run the vCPU.
    type Error: fmt::Display;

    /// Run the guest until the next VM exit.
    ///
    /// Port I/O and MMIO are served through `io` before this returns. Port
    /// I/O is reported as [`VmExit::Io`], MMIO that a device served as
    /// [`VmExit::MmioDevice`], and MMIO that no device claimed as
    /// [`VmExit::Mmio`].
    ///
    /// # Errors
    ///
    /// The backend could not run the vCPU, e.g. the host interrupted it or
    /// rejected the guest state.
    fn run(&mut self, io: &mut dyn GuestIo) -> Result<VmExit, Self::Error>;
}

/// A request, shared between vCPUs, to stop running the VM.
pub trait StopFlag {
    /// Whether the VM should stop.
    fn is_set(&self) -> bool;
}

/// A [`StopFlag`] that is never set, for single-vCPU runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverStop;

impl StopFlag for NeverStop {
    fn is_set(&self) -> bool {
        false
    }
}

/// The flag SMP runners share. Acquire pairs with the setter's release, so a
/// vCPU that sees the flag also sees everything written before it was set.
impl StopFlag for core::sync::atomic::AtomicBool {
    fn is_set(&self) -> bool {
        self.load(core::sync::atomic::Ordering::Acquire)
    }
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
    /// The guest executed `HLT` and the backend reported it. With an
    /// in-kernel interrupt controller the backend handles `HLT` itself; when
    /// it is reported, the guest can never resume, so this ends the run
    /// normally.
    Halted,
    /// The guest requested shutdown or triple-faulted.
    Shutdown,
    /// MMIO to a guest-physical address with neither RAM nor a device.
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
    /// Another vCPU stopped the VM (the [`StopFlag`] was set).
    Requested,
}

impl StopReason {
    /// Whether this ends the VM normally (`Stopped`) rather than as a failure.
    /// [`StopReason::Requested`] is neither; see [`vm_stop_event`].
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

/// Run `vcpu` until its guest stops or `stop` is set. Does not touch the
/// lifecycle; the caller combines per-vCPU results with [`vm_stop_event`].
pub fn run_vcpu_until<V: Vcpu>(
    vcpu: &mut V,
    io: &mut dyn GuestIo,
    limits: RunLimits,
    stop: &dyn StopFlag,
) -> RunOutcome {
    let mut exits: u64 = 0;
    let mut trace = Vec::new();
    let reason = loop {
        if stop.is_set() {
            break StopReason::Requested;
        }
        if exits >= limits.max_exits {
            break StopReason::ExitLimit;
        }
        let exit = match vcpu.run(io) {
            Ok(exit) => exit,
            // A vCPU kicked out of the backend because another one stopped
            // the VM is not a backend failure.
            Err(_) if stop.is_set() => break StopReason::Requested,
            Err(e) => break StopReason::BackendError(alloc::format!("{e}")),
        };
        exits = exits.saturating_add(1);
        if trace.len() < limits.trace_capacity {
            trace.push(exit);
        }
        match exit {
            VmExit::Io(_) | VmExit::MmioDevice(_) | VmExit::Cpuid { .. } => {}
            VmExit::Hlt => break StopReason::Halted,
            VmExit::Shutdown => break StopReason::Shutdown,
            VmExit::Mmio(access) => break StopReason::UnhandledMmio(access),
            VmExit::Unhandled { raw_reason } => break StopReason::UnhandledExit { raw_reason },
        }
    };
    RunOutcome {
        stop: reason,
        exits,
        trace,
    }
}

/// The lifecycle event for a whole VM whose vCPUs stopped for `reasons`.
///
/// `Requested` stops are followers and decide nothing. The VM stops normally
/// only if at least one vCPU stopped normally and none stopped abnormally.
/// If every vCPU reports `Requested`, nothing explains the stop, and the VM
/// is treated as failed.
#[must_use]
pub fn vm_stop_event(reasons: &[StopReason]) -> VmEvent {
    let mut decisive = reasons
        .iter()
        .filter(|r| **r != StopReason::Requested)
        .peekable();
    if decisive.peek().is_none() {
        return VmEvent::Fail;
    }
    if decisive.all(StopReason::is_normal) {
        VmEvent::Stop
    } else {
        VmEvent::Fail
    }
}

/// Run `vcpu` until the guest stops, then move `lifecycle` to `Stopped`
/// (normal stop) or `Failed` (anything else).
///
/// # Errors
///
/// Returns [`NotRunning`] if `lifecycle` is not in [`VmState::Running`]; the
/// vCPU is not run.
pub fn run_vcpu<V: Vcpu>(
    vcpu: &mut V,
    io: &mut dyn GuestIo,
    lifecycle: &mut VmLifecycle,
    limits: RunLimits,
) -> Result<RunOutcome, NotRunning> {
    if lifecycle.state() != VmState::Running {
        return Err(NotRunning(lifecycle.state()));
    }
    let outcome = run_vcpu_until(vcpu, io, limits, &NeverStop);
    // Running accepts both Stop and Fail, and the state was checked above.
    let _ = lifecycle.apply(vm_stop_event(core::slice::from_ref(&outcome.stop)));
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_stop_event_combines_vcpu_reasons() {
        use StopReason as R;
        assert_eq!(vm_stop_event(&[R::Shutdown]), VmEvent::Stop);
        assert_eq!(
            vm_stop_event(&[R::Shutdown, R::Requested, R::Requested]),
            VmEvent::Stop
        );
        assert_eq!(vm_stop_event(&[R::Requested, R::Halted]), VmEvent::Stop);
        assert_eq!(vm_stop_event(&[R::Shutdown, R::ExitLimit]), VmEvent::Fail);
        assert_eq!(
            vm_stop_event(&[R::Requested, R::BackendError("x".into())]),
            VmEvent::Fail
        );
        assert_eq!(
            vm_stop_event(&[R::Requested, R::Requested]),
            VmEvent::Fail,
            "nothing decisive"
        );
        assert_eq!(vm_stop_event(&[]), VmEvent::Fail);
    }
}
