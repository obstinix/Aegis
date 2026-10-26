//! Running a guest's vCPUs, one thread each (roadmap M4h).
//!
//! Every vCPU runs [`aegis_core::run_vcpu_until`] on its own scoped thread
//! against a [`DeviceBus`] shared behind a mutex. The first vCPU whose run
//! ends for a reason of its own (shutdown, failure, exit limit) sets a shared
//! stop flag. The other vCPUs may be blocked inside `KVM_RUN`: with an
//! in-kernel LAPIC, a halted vCPU waits in the kernel. So the coordinating
//! thread sends each of them `SIGRTMIN` (a raw `tgkill`), which has a no-op handler and
//! makes `KVM_RUN` return `EINTR`. The vCPU then sees the flag and stops
//! with [`StopReason::Requested`].
//!
//! The kick is re-sent every [`KICK_INTERVAL`] until every vCPU has
//! finished. That closes the race where a signal arrives just before a
//! vCPU re-enters `KVM_RUN`, because vCPUs also check the flag before each
//! entry.
//!
//! With `sandbox` set, each vCPU thread installs the run-loop seccomp
//! filter on itself before running the guest, and the coordinator does the
//! same once it knows every thread's handle.
//!
//! The stop flag belongs to the caller, so host threads that serve the guest
//! (such as the stdin worker in [`crate::input`]) see the stop at the same
//! moment the vCPUs do. Registered as [`HostThread`]s, they are kicked and
//! waited for like the vCPUs, so none outlives the run. A device those
//! threads also reach is attached through [`SharedMmio`]. The locks are then
//! always taken bus first, device second, and host threads take only the
//! device lock, so the two cannot deadlock.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use aegis_core::{
    AccessSize, DeviceBus, MmioDevice, MmioIo, PortIo, RunLimits, RunOutcome, StopReason,
    run_vcpu_until,
};
use vmm_sys_util::signal::{SIGRTMIN, register_signal_handler};

use crate::{KvmError, KvmVcpu};

/// How often stopped vCPUs' siblings are kicked again until they exit.
pub const KICK_INTERVAL: Duration = Duration::from_millis(10);

/// What one vCPU's run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcpuResult {
    /// vCPU index.
    pub id: u64,
    /// Why and how its run ended.
    pub outcome: RunOutcome,
    /// The backend's description of an unhandled exit, if that ended the run.
    pub detail: Option<String>,
}

/// A [`DeviceBus`] shared between vCPU threads; each access takes the lock.
struct SharedBus<'a, 'd>(&'a Mutex<DeviceBus<'d>>);

impl<'d> SharedBus<'_, 'd> {
    fn lock(&self) -> MutexGuard<'_, DeviceBus<'d>> {
        // A panicking device thread poisons the lock. The bus holds no
        // invariants a panic could break half-way, so keep serving.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl PortIo for SharedBus<'_, '_> {
    fn read(&mut self, port: u16, size: AccessSize) -> u32 {
        self.lock().read(port, size)
    }
    fn write(&mut self, port: u16, size: AccessSize, value: u32) {
        self.lock().write(port, size, value);
    }
}

impl MmioIo for SharedBus<'_, '_> {
    fn mmio_read(&mut self, gpa: u64, size: AccessSize) -> Option<u64> {
        self.lock().mmio_read(gpa, size)
    }
    fn mmio_write(&mut self, gpa: u64, size: AccessSize, value: u64) -> bool {
        self.lock().mmio_write(gpa, size, value)
    }
}

/// A host thread that serves the guest (such as the stdin worker) and may
/// block in a syscall. [`run`] treats it like a vCPU at the end: once the
/// guest stops, it kicks the thread with `SIGRTMIN` until the thread is done
/// (a blocked `read` or `poll` returns `EINTR`), and only then returns.
///
/// The thread calls [`HostThread::enter`] as its first action and holds the
/// guard until it finishes; dropping the guard, even while unwinding from a
/// panic, marks it done. A `HostThread` passed to [`run`] must belong to a
/// thread that has been spawned, or `run` waits for it forever.
#[derive(Debug, Default)]
pub struct HostThread {
    /// Thread ID once entered, 0 before.
    tid: AtomicI32,
    done: AtomicBool,
}

/// Marks its [`HostThread`] done when dropped.
#[derive(Debug)]
pub struct HostThreadGuard<'a>(&'a HostThread);

impl HostThread {
    /// A thread that has not entered yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tid: AtomicI32::new(0),
            done: AtomicBool::new(false),
        }
    }

    /// Record the calling thread as this host thread. Call it before
    /// installing a seccomp filter, since it uses `gettid`.
    #[must_use]
    pub fn enter(&self) -> HostThreadGuard<'_> {
        self.tid.store(current_tid(), Ordering::Release);
        HostThreadGuard(self)
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

impl Drop for HostThreadGuard<'_> {
    fn drop(&mut self) {
        self.0.done.store(true, Ordering::Release);
    }
}

/// An MMIO device behind its own mutex, so a host thread can reach it while
/// vCPUs use it through the bus. Attach this to the [`DeviceBus`]; the host
/// thread locks the same mutex directly.
#[derive(Debug)]
pub struct SharedMmio<'a, T>(pub &'a Mutex<T>);

impl<T> SharedMmio<'_, T> {
    fn lock(&self) -> MutexGuard<'_, T> {
        // As for the bus: keep serving after a panic elsewhere.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: MmioDevice> MmioDevice for SharedMmio<'_, T> {
    fn read(&mut self, offset: u64, size: AccessSize) -> u64 {
        self.lock().read(offset, size)
    }
    fn write(&mut self, offset: u64, size: AccessSize, value: u64) {
        self.lock().write(offset, size, value);
    }
}

/// The kick handler: does nothing. Delivery alone makes `KVM_RUN` return `EINTR`.
extern "C" fn on_kick(_num: libc::c_int, _info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {}

#[allow(unsafe_code)]
fn current_tid() -> libc::pid_t {
    // SAFETY: gettid has no preconditions and cannot fail.
    unsafe { libc::gettid() }
}

/// Send `SIGRTMIN` to thread `tid` of process `pid` with a raw `tgkill`.
///
/// Not `pthread_kill`: glibc's implementation also calls `getpid`, which the
/// seccomp filter does not allow (found from the audit record of the
/// `SIGSYS` kill: `syscall=39`). The raw call is exactly the one the filter
/// allows, whatever the C library does.
#[allow(unsafe_code)]
fn kick(pid: libc::pid_t, tid: libc::pid_t) {
    // SAFETY: tgkill takes three integers and touches no memory of ours.
    // `pid` is this process, so the signal can only reach one of its
    // threads, and no thread can be created to reuse `tid` while the run
    // loop's seccomp filter forbids `clone`. SIGRTMIN has a no-op handler
    // installed (see `run`), so delivery has no effect beyond interrupting a
    // blocking syscall such as `KVM_RUN`. A failed delivery (e.g. `ESRCH`
    // for a thread that has just exited) is harmless: the kick is resent
    // until the thread reports that it finished.
    let _ = unsafe {
        libc::syscall(
            libc::SYS_tgkill,
            libc::c_long::from(pid),
            libc::c_long::from(tid),
            libc::c_long::from(SIGRTMIN()),
        )
    };
}

/// Run every vCPU on its own thread until the guest stops. Results are in
/// vCPU order.
///
/// `stop` must start clear. The first vCPU that stops for its own reason
/// sets it, and so does a coordinator that cannot sandbox itself. Other
/// threads may watch it to learn that the guest is stopping. It is set
/// when this returns `Ok`.
///
/// `host_threads` are waited for and kicked like the vCPUs once `stop` is
/// set (see [`HostThread`]), so when this returns `Ok` none of them is still
/// serving the guest.
///
/// # Errors
///
/// [`KvmError::Kvm`] if the kick signal handler cannot be installed; no
/// vCPU runs then. A vCPU thread that cannot install its seccomp filter
/// does not run the guest; it reports [`StopReason::BackendError`] and stops
/// the others.
pub fn run(
    vcpus: Vec<KvmVcpu<'_>>,
    bus: &Mutex<DeviceBus<'_>>,
    limits: RunLimits,
    sandbox: bool,
    stop: &AtomicBool,
    host_threads: &[&HostThread],
) -> Result<Vec<VcpuResult>, KvmError> {
    register_signal_handler(SIGRTMIN(), on_kick).map_err(|source| KvmError::Kvm {
        op: "install the SIGRTMIN kick handler",
        source,
    })?;

    let count = vcpus.len();
    // Read now: getpid is not allowed once the filter is installed.
    let pid = libc::pid_t::try_from(std::process::id())
        .map_err(|_| KvmError::Unsupported("a process ID that does not fit pid_t"))?;
    let finished: Vec<AtomicBool> = (0..count).map(|_| AtomicBool::new(false)).collect();
    let (tx, rx) = mpsc::channel::<(usize, libc::pid_t)>();

    let results = std::thread::scope(|s| {
        let handles: Vec<_> = vcpus
            .into_iter()
            .enumerate()
            .map(|(index, mut vcpu)| {
                let (tx, done) = (tx.clone(), &finished[index]);
                s.spawn(move || {
                    let _ = tx.send((index, current_tid()));
                    drop(tx);
                    let outcome = match sandbox
                        .then(crate::seccomp::apply_run_loop_filter)
                        .transpose()
                    {
                        Ok(_) => run_vcpu_until(&mut vcpu, &mut SharedBus(bus), limits, stop),
                        Err(e) => RunOutcome {
                            stop: StopReason::BackendError(e.to_string()),
                            exits: 0,
                            trace: Vec::new(),
                        },
                    };
                    if outcome.stop != StopReason::Requested {
                        stop.store(true, Ordering::Release);
                    }
                    done.store(true, Ordering::Release);
                    VcpuResult {
                        id: vcpu.id(),
                        detail: vcpu.last_unhandled_detail().map(str::to_owned),
                        outcome,
                    }
                })
            })
            .collect();
        drop(tx);

        let mut threads = vec![None; count];
        for (index, thread) in rx.iter().take(count) {
            threads[index] = Some(thread);
        }
        if sandbox && let Err(e) = crate::seccomp::apply_run_loop_filter() {
            // The coordinator cannot be sandboxed: stop the guest rather than
            // run it with an unfiltered thread that can signal the vCPUs.
            eprintln!("aegis: {e}; stopping the guest");
            stop.store(true, Ordering::Release);
        }
        while !finished.iter().all(|f| f.load(Ordering::Acquire))
            || !host_threads.iter().all(|h| h.is_done())
        {
            if stop.load(Ordering::Acquire) {
                for (done, thread) in finished.iter().zip(&threads) {
                    if let (false, Some(tid)) = (done.load(Ordering::Acquire), thread) {
                        kick(pid, *tid);
                    }
                }
                for host in host_threads.iter().filter(|h| !h.is_done()) {
                    // 0: not entered yet. It will see the stop flag first.
                    let tid = host.tid.load(Ordering::Acquire);
                    if tid != 0 {
                        kick(pid, tid);
                    }
                }
            }
            std::thread::sleep(KICK_INTERVAL);
        }
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect::<Vec<_>>()
    });
    // Already set by whichever vCPU stopped first; also covers no vCPUs.
    stop.store(true, Ordering::Release);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    /// The mechanism `run` uses on host threads, without a VM: a registered
    /// thread blocked in `read` is kicked out with `EINTR`, and its guard
    /// marks it done.
    #[test]
    fn a_host_thread_blocked_in_read_is_kicked_out_and_marked_done() {
        register_signal_handler(SIGRTMIN(), on_kick).unwrap();
        let pid = libc::pid_t::try_from(std::process::id()).unwrap();
        // The writer stays open, so the read can only end by a signal.
        let (_writer, mut reader) = UnixStream::pair().unwrap();
        let host = HostThread::new();
        assert!(!host.is_done());
        std::thread::scope(|s| {
            s.spawn(|| {
                let _guard = host.enter();
                let err = reader.read(&mut [0u8; 1]).unwrap_err();
                assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
            });
            let started = Instant::now();
            while !host.is_done() {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "the blocked thread was never kicked out"
                );
                // Kicks that land before the read starts do nothing; the
                // next one interrupts it, exactly as in `run`.
                let tid = host.tid.load(Ordering::Acquire);
                if tid != 0 {
                    kick(pid, tid);
                }
                std::thread::sleep(KICK_INTERVAL);
            }
        });
    }
}
