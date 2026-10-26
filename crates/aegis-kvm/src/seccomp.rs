//! Seccomp allowlist for the VM run loop (`research/security-analysis.md` §3.1).
//!
//! Applied after all setup (files read, guest memory mapped, vCPU
//! configured), the filter leaves the process able to do only what the run
//! loop and its orderly exit need. Everything else kills the whole process
//! with `SIGSYS`:
//!
//! - `ioctl`, but only `KVM_RUN` and `KVM_IRQ_LINE` (virtio interrupts): no
//!   other KVM, terminal or device control.
//! - `tgkill`, but only to this process (`tgid` = own PID) with `SIGRTMIN`:
//!   the SMP runner kicking its own vCPU threads out of `KVM_RUN`.
//! - `clock_nanosleep` and `nanosleep`, for the SMP coordinator's kick
//!   interval.
//! - Memory management (`brk`, `mmap`, `munmap`, `mremap`, `madvise`) for
//!   heap growth and teardown.
//! - `write` to stdout and stderr only, for printing results after the run.
//! - `futex` and `sched_yield`, used by std locks around stdout.
//! - `close`, `exit`, `exit_group`, `rt_sigreturn`, `rt_sigprocmask` and
//!   `sigaltstack`, for teardown and signal return.
//! - `fcntl`, but only `F_GETFD`: a read-only flag query. Debug builds of Rust
//!   std issue it when dropping a file descriptor (`debug_assert_fd_is_open`),
//!   which happens when the VM and vCPU fds close at teardown. Found with gdb
//!   `catch syscall fcntl` after the filter killed a debug build at teardown.
//!
//! Not allowed: opening, reading or creating files; sockets; process or
//! thread creation; signalling other processes; any KVM ioctl except the two
//! above; and any other `fcntl` command (no `F_DUPFD`, `F_SETFL` or locks).
//!
//! The host-input worker ([`crate::input`]) gets [`input_worker_filter`]:
//! the same list plus `read` from fd 0 and `poll`/`ppoll`, nothing else.

use std::collections::BTreeMap;
use std::fmt;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// `KVM_RUN` = `_IO(KVMIO, 0x80)` with `KVMIO` = 0xAE.
const KVM_RUN: u64 = 0xAE80;
/// `KVM_IRQ_LINE` = `_IOW(KVMIO, 0x61, struct kvm_irq_level)` (8 bytes).
const KVM_IRQ_LINE: u64 = 0x4008_AE61;

/// The filter could not be built or installed.
#[derive(Debug)]
pub struct SeccompError(String);

impl fmt::Display for SeccompError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "seccomp: {}", self.0)
    }
}

impl std::error::Error for SeccompError {}

fn err(e: impl fmt::Display) -> SeccompError {
    SeccompError(e.to_string())
}

fn arg_eq(index: u8, value: u64) -> Result<SeccompRule, SeccompError> {
    SeccompRule::new(vec![
        SeccompCondition::new(index, SeccompCmpArgLen::Qword, SeccompCmpOp::Eq, value)
            .map_err(err)?,
    ])
    .map_err(err)
}

/// Build the run-loop allowlist as a BPF program.
///
/// # Errors
///
/// [`SeccompError`] if the filter cannot be compiled.
pub fn run_loop_filter() -> Result<BpfProgram, SeccompError> {
    compile(run_loop_rules()?)
}

/// The run-loop allowlist plus what [`crate::input::forward`] needs on
/// stdin: `read` from fd 0 only, and `poll`/`ppoll` (glibc may use either).
/// Nothing else is added, so the input worker cannot open, write or signal
/// anything the vCPU threads cannot.
///
/// # Errors
///
/// [`SeccompError`] if the filter cannot be compiled.
pub fn input_worker_filter() -> Result<BpfProgram, SeccompError> {
    compile(input_worker_rules()?)
}

fn input_worker_rules() -> Result<BTreeMap<i64, Vec<SeccompRule>>, SeccompError> {
    let mut rules = run_loop_rules()?;
    let stdin = u64::try_from(crate::input::STDIN).map_err(err)?;
    rules.insert(libc::SYS_read, vec![arg_eq(0, stdin)?]);
    rules.insert(libc::SYS_poll, Vec::new());
    rules.insert(libc::SYS_ppoll, Vec::new());
    Ok(rules)
}

/// Install [`input_worker_filter`] on the calling thread. Irreversible.
///
/// # Errors
///
/// [`SeccompError`] if the filter cannot be built or the kernel rejects it.
pub fn apply_input_worker_filter() -> Result<(), SeccompError> {
    let program = input_worker_filter()?;
    seccompiler::apply_filter(&program).map_err(err)
}

fn compile(rules: BTreeMap<i64, Vec<SeccompRule>>) -> Result<BpfProgram, SeccompError> {
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::KillProcess,
        SeccompAction::Allow,
        TargetArch::x86_64,
    )
    .map_err(err)?;
    BpfProgram::try_from(filter).map_err(err)
}

fn run_loop_rules() -> Result<BTreeMap<i64, Vec<SeccompRule>>, SeccompError> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    rules.insert(
        libc::SYS_ioctl,
        vec![arg_eq(1, KVM_RUN)?, arg_eq(1, KVM_IRQ_LINE)?],
    );
    let own_pid = u64::from(std::process::id());
    let kick = u64::try_from(vmm_sys_util::signal::SIGRTMIN()).map_err(err)?;
    rules.insert(
        libc::SYS_tgkill,
        vec![
            SeccompRule::new(vec![
                SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, own_pid)
                    .map_err(err)?,
                SeccompCondition::new(2, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, kick)
                    .map_err(err)?,
            ])
            .map_err(err)?,
        ],
    );
    rules.insert(libc::SYS_write, vec![arg_eq(0, 1)?, arg_eq(0, 2)?]);
    let f_getfd = u64::try_from(libc::F_GETFD).map_err(err)?;
    rules.insert(libc::SYS_fcntl, vec![arg_eq(1, f_getfd)?]);
    for syscall in [
        libc::SYS_brk,
        libc::SYS_mmap,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_close,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_rt_sigreturn,
        libc::SYS_rt_sigprocmask,
        libc::SYS_sigaltstack,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
    ] {
        // An empty rule list means "allow unconditionally".
        rules.insert(syscall, Vec::new());
    }
    Ok(rules)
}

/// Install [`run_loop_filter`] on the calling thread. Irreversible: the
/// thread (and any it spawns) stays filtered until the process exits.
///
/// # Errors
///
/// [`SeccompError`] if the filter cannot be built or the kernel rejects it.
pub fn apply_run_loop_filter() -> Result<(), SeccompError> {
    let program = run_loop_filter()?;
    seccompiler::apply_filter(&program).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_compiles_to_a_nonempty_program() {
        let program = run_loop_filter().unwrap();
        assert!(
            program.len() > 10,
            "BPF program has {} instructions",
            program.len()
        );
    }

    #[test]
    fn input_worker_filter_adds_only_stdin_reads_and_poll() {
        let base = run_loop_rules().unwrap();
        let worker = input_worker_rules().unwrap();
        let added: Vec<i64> = worker
            .keys()
            .filter(|k| !base.contains_key(k))
            .copied()
            .collect();
        // Keys iterate in syscall-number order: read 0, poll 7, ppoll 271.
        assert_eq!(added, [libc::SYS_read, libc::SYS_poll, libc::SYS_ppoll]);
        assert_eq!(
            worker[&libc::SYS_read],
            vec![arg_eq(0, 0).unwrap()],
            "read only from fd 0"
        );
        assert!(
            base.iter().all(|(k, v)| worker.get(k) == Some(v)),
            "every run-loop rule is unchanged"
        );
        assert!(
            !base.contains_key(&libc::SYS_read),
            "vCPU threads cannot read"
        );
        assert!(input_worker_filter().unwrap().len() > run_loop_filter().unwrap().len());
    }
}
