//! Roadmap M2 acceptance tests: real guests under KVM.
//!
//! The guests are the NASM sources in `tests/guests/`, assembled at test time.
//! Every test needs an openable `/dev/kvm` and `nasm`, so they are `#[ignore]`d:
//! a plain `cargo test` reports them as ignored, never as passed. Run them with
//!
//! ```text
//! cargo test -p aegis-kvm -- --include-ignored
//! ```
//!
//! Tests take a process-wide lock because the file-descriptor leak check
//! counts every fd in the process.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use aegis_core::{
    AccessSize, DeviceBus, GuestMemoryMap, IoAccess, MemoryRegion, MmioAccess, Permissions,
    RunLimits, RunOutcome, StopReason, VmEvent, VmExit, VmLifecycle, VmState, run_vcpu,
};
use aegis_devices::Serial16550;
use aegis_kvm::{KvmError, KvmVm};

const MIB: u64 = 1024 * 1024;
const LOAD_ADDR: u64 = 0x1000;
const COM1: u16 = 0x3F8;
/// The `message` string in `tests/guests/hello16.asm`, without its NUL.
const MESSAGE: &[u8] = b"Aegis M2: hello from a KVM guest\n";

static SERIAL: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Assemble `tests/guests/<name>.asm` into a flat binary.
fn assemble(name: &str) -> Vec<u8> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/guests")
        .join(format!("{name}.asm"));
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.bin"));
    let status = Command::new("nasm")
        .args(["-f", "bin", "-w+all", "-o"])
        .arg(&out)
        .arg(&src)
        .status()
        .unwrap_or_else(|e| panic!("cannot run nasm ({e}); install NASM to run the KVM tests"));
    assert!(status.success(), "nasm failed on {}", src.display());
    std::fs::read(&out).unwrap()
}

fn open_fds() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

fn ram(size: u64) -> GuestMemoryMap {
    GuestMemoryMap::new(
        &[MemoryRegion {
            start: 0,
            size,
            permissions: Permissions::RWX,
        }],
        2 * MIB,
    )
    .unwrap()
}

fn out_exit(port: u16, byte: u8) -> VmExit {
    VmExit::Io(IoAccess::new(port, AccessSize::Byte, Some(u32::from(byte))).unwrap())
}

fn in_exit(port: u16) -> VmExit {
    VmExit::Io(IoAccess::new(port, AccessSize::Byte, None).unwrap())
}

struct Run {
    outcome: RunOutcome,
    serial: Vec<u8>,
    unassigned: aegis_core::UnassignedLog,
    lifecycle: VmLifecycle,
    detail: Option<String>,
}

/// Create a VM with `map`, load `guest` at 0x1000, enter it in real mode at
/// 0000:1000 with a UART on COM1, run it, and tear everything down.
fn run_guest(map: GuestMemoryMap, guest: &[u8]) -> Run {
    let mut lifecycle = VmLifecycle::new();
    let vm = KvmVm::new(map).unwrap();
    vm.write_guest(LOAD_ADDR, guest).unwrap();
    let mut vcpu = vm.create_vcpu(0).unwrap();
    vcpu.set_real_mode_entry(0, 0x1000).unwrap();
    lifecycle.apply(VmEvent::Configure).unwrap();
    lifecycle.apply(VmEvent::Start).unwrap();

    let mut uart = Serial16550::default();
    let (outcome, unassigned) = {
        let mut bus = DeviceBus::new();
        bus.attach_port(COM1, Serial16550::PORTS, &mut uart)
            .unwrap();
        let outcome = run_vcpu(&mut vcpu, &mut bus, &mut lifecycle, RunLimits::default()).unwrap();
        (outcome, bus.unassigned().clone())
    };
    let detail = vcpu.last_unhandled_detail().map(str::to_owned);
    drop(vcpu);
    drop(vm);
    lifecycle.apply(VmEvent::Destroy).unwrap();
    Run {
        outcome,
        serial: uart.output().to_vec(),
        unassigned,
        lifecycle,
        detail,
    }
}

#[test]
#[ignore = "requires /dev/kvm and nasm; run with --include-ignored"]
fn hello16_prints_through_the_uart_and_halts() {
    let _guard = lock();
    let guest = assemble("hello16");
    let fds_before = open_fds();

    let run = run_guest(ram(2 * MIB), &guest);

    let expected: Vec<VmExit> = MESSAGE
        .iter()
        .map(|&b| out_exit(COM1, b))
        .chain([VmExit::Hlt])
        .collect();
    assert_eq!(
        run.outcome.stop,
        StopReason::Halted,
        "detail: {:?}",
        run.detail
    );
    assert_eq!(run.outcome.trace, expected, "exact exit sequence");
    assert_eq!(run.outcome.exits, u64::try_from(expected.len()).unwrap());
    assert_eq!(run.serial, MESSAGE);
    assert_eq!(run.unassigned.count, 0);
    assert_eq!(run.lifecycle.state(), VmState::Destroyed);
    assert_eq!(
        open_fds(),
        fds_before,
        "VM and vCPU file descriptors were all closed"
    );
}

#[test]
#[ignore = "requires /dev/kvm and nasm; run with --include-ignored"]
fn unassigned_ports_are_logged_read_as_all_ones_and_the_guest_continues() {
    let _guard = lock();
    let run = run_guest(ram(2 * MIB), &assemble("unassigned_port"));

    assert_eq!(
        run.outcome.stop,
        StopReason::Halted,
        "detail: {:?}",
        run.detail
    );
    assert_eq!(
        run.outcome.trace,
        [
            out_exit(0x80, 0x12),
            in_exit(0x404),
            out_exit(COM1, 0xFF),
            out_exit(COM1, b'k'),
            VmExit::Hlt
        ]
    );
    assert_eq!(
        run.serial,
        [0xFF, b'k'],
        "the guest saw 0xFF from the unassigned read"
    );
    assert_eq!(run.unassigned.count, 2);
    assert_eq!(
        (
            run.unassigned.first[0].port(),
            run.unassigned.first[0].write_value()
        ),
        (0x80, Some(0x12))
    );
    assert_eq!(
        (
            run.unassigned.first[1].port(),
            run.unassigned.first[1].write_value()
        ),
        (0x404, None)
    );
    assert_eq!(run.lifecycle.state(), VmState::Destroyed);
}

#[test]
#[ignore = "requires /dev/kvm and nasm; run with --include-ignored"]
fn mmio_to_unbacked_memory_fails_the_vm_with_the_access() {
    let _guard = lock();
    // RAM only below 0x80000; the guest writes to 0x90000.
    let run = run_guest(ram(0x8_0000), &assemble("mmio_unmapped"));

    let access = MmioAccess::new(0x9_0000, AccessSize::Byte, Some(0x42)).unwrap();
    assert_eq!(run.outcome.stop, StopReason::UnhandledMmio(access));
    assert_eq!(run.outcome.trace, [VmExit::Mmio(access)]);
    assert!(run.serial.is_empty());
    // Failed is reached before teardown; Destroy is valid from Failed.
    assert_eq!(run.lifecycle.state(), VmState::Destroyed);
}

#[test]
#[ignore = "requires /dev/kvm and nasm; run with --include-ignored"]
fn host_writes_outside_guest_memory_are_rejected() {
    let _guard = lock();
    let vm = KvmVm::new(ram(0x8_0000)).unwrap();
    assert!(vm.write_guest(0x7_FFFF, &[1]).is_ok());
    assert!(matches!(
        vm.write_guest(0x7_FFFF, &[1, 2]),
        Err(KvmError::OutOfBounds {
            gpa: 0x7_FFFF,
            len: 2
        })
    ));
    assert!(matches!(
        vm.write_guest(0x10_0000, &[1]),
        Err(KvmError::OutOfBounds { .. })
    ));
    let mut back = [0u8; 1];
    vm.read_guest(0x7_FFFF, &mut back).unwrap();
    assert_eq!(back, [1]);
}

/// Rejected before /dev/kvm is opened, so this runs without KVM access.
#[test]
fn non_executable_memory_is_refused_rather_than_silently_ignored() {
    let map = GuestMemoryMap::new(
        &[MemoryRegion {
            start: 0,
            size: MIB,
            permissions: Permissions {
                read: true,
                write: true,
                execute: false,
            },
        }],
        2 * MIB,
    )
    .unwrap();
    assert!(matches!(KvmVm::new(map), Err(KvmError::Unsupported(_))));
}
