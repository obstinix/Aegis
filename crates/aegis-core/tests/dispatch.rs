//! `run_vcpu` driven by the scripted mock backend: exit handling, lifecycle
//! transitions, limits, and port routing.

use aegis_core::mock::{ScriptedVcpu, Step, io_exit};
use aegis_core::{
    AccessSize, DeviceBus, MmioAccess, PortDevice, RunLimits, StopReason, VmEvent, VmExit,
    VmLifecycle, VmState, run_vcpu, run_vcpu_until,
};

fn running() -> VmLifecycle {
    let mut vm = VmLifecycle::new();
    vm.apply(VmEvent::Configure).unwrap();
    vm.apply(VmEvent::Start).unwrap();
    vm
}

/// Collects bytes written at offset 0; reads at offset 5 return 0x60.
#[derive(Default)]
struct Sink(Vec<u8>);

impl PortDevice for Sink {
    fn read(&mut self, offset: u16, _: AccessSize) -> u32 {
        if offset == 5 { 0x60 } else { 0 }
    }
    fn write(&mut self, offset: u16, _: AccessSize, value: u32) {
        if offset == 0 {
            self.0.push(value.to_le_bytes()[0]);
        }
    }
}

fn out(port: u16, byte: u8) -> Step {
    Step::Out {
        port,
        size: AccessSize::Byte,
        value: u32::from(byte),
    }
}

#[test]
fn halt_after_io_stops_normally_with_full_trace() {
    let mut sink = Sink::default();
    let mut vcpu = ScriptedVcpu::new(
        b"hi"
            .iter()
            .map(|&b| out(0x3F8, b))
            .chain([Step::Exit(VmExit::Hlt)]),
    );
    let mut vm = running();
    let outcome = {
        let mut bus = DeviceBus::new();
        bus.attach_port(0x3F8, 8, &mut sink).unwrap();
        run_vcpu(&mut vcpu, &mut bus, &mut vm, RunLimits::default()).unwrap()
    };
    assert_eq!(outcome.stop, StopReason::Halted);
    assert_eq!(outcome.exits, 3);
    assert_eq!(
        outcome.trace,
        [
            io_exit(0x3F8, AccessSize::Byte, Some(u32::from(b'h'))),
            io_exit(0x3F8, AccessSize::Byte, Some(u32::from(b'i'))),
            VmExit::Hlt
        ]
    );
    assert_eq!(sink.0, b"hi");
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(vm.apply(VmEvent::Destroy), Ok(VmState::Destroyed));
}

#[test]
fn shutdown_stops_normally() {
    let mut vcpu = ScriptedVcpu::new([Step::Exit(VmExit::Shutdown)]);
    let mut vm = running();
    let outcome = run_vcpu(
        &mut vcpu,
        &mut DeviceBus::new(),
        &mut vm,
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(outcome.stop, StopReason::Shutdown);
    assert_eq!(vm.state(), VmState::Stopped);
}

#[test]
fn unassigned_port_access_is_logged_and_the_guest_continues() {
    let mut sink = Sink::default();
    let mut vcpu = ScriptedVcpu::new([
        out(0x80, 0x12),
        Step::In {
            port: 0x404,
            size: AccessSize::Byte,
        },
        out(0x3F8, b'k'),
        Step::Exit(VmExit::Hlt),
    ]);
    let mut vm = running();
    let mut bus = DeviceBus::new();
    bus.attach_port(0x3F8, 8, &mut sink).unwrap();
    let outcome = run_vcpu(&mut vcpu, &mut bus, &mut vm, RunLimits::default()).unwrap();
    assert_eq!(
        outcome.stop,
        StopReason::Halted,
        "guest kept running after unassigned accesses"
    );
    let log = bus.unassigned();
    assert_eq!(log.count, 2);
    assert_eq!(
        (log.first[0].port(), log.first[0].write_value()),
        (0x80, Some(0x12))
    );
    assert_eq!(
        (log.first[1].port(), log.first[1].write_value()),
        (0x404, None)
    );
    drop(bus);
    assert_eq!(vcpu.reads, [0xFF], "unassigned read served all ones");
    assert_eq!(sink.0, b"k");
}

#[test]
fn unknown_exit_fails_the_vm_with_the_raw_reason() {
    let mut vcpu = ScriptedVcpu::new([
        out(0x80, 1),
        Step::Exit(VmExit::Unhandled { raw_reason: 0xBEEF }),
    ]);
    let mut vm = running();
    let outcome = run_vcpu(
        &mut vcpu,
        &mut DeviceBus::new(),
        &mut vm,
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(
        outcome.stop,
        StopReason::UnhandledExit { raw_reason: 0xBEEF }
    );
    assert_eq!(vm.state(), VmState::Failed);
    assert_eq!(vcpu.remaining(), 0);
}

#[test]
fn mmio_without_a_device_fails_the_vm_with_the_access() {
    let access = MmioAccess::new(0x9_0000, AccessSize::Byte, Some(0x42)).unwrap();
    let mut vcpu = ScriptedVcpu::new([Step::Exit(VmExit::Mmio(access))]);
    let mut vm = running();
    let outcome = run_vcpu(
        &mut vcpu,
        &mut DeviceBus::new(),
        &mut vm,
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(outcome.stop, StopReason::UnhandledMmio(access));
    assert_eq!(vm.state(), VmState::Failed);
}

#[test]
fn backend_error_fails_the_vm_with_its_message() {
    let mut vcpu = ScriptedVcpu::new([]);
    let mut vm = running();
    let outcome = run_vcpu(
        &mut vcpu,
        &mut DeviceBus::new(),
        &mut vm,
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(
        outcome.stop,
        StopReason::BackendError("mock vCPU script exhausted".to_owned())
    );
    assert_eq!(outcome.exits, 0);
    assert_eq!(vm.state(), VmState::Failed);
}

#[test]
fn exit_limit_stops_an_exit_storm_and_bounds_the_trace() {
    let mut vcpu = ScriptedVcpu::new((0..1000).map(|_| out(0x80, 0)));
    let mut vm = running();
    let limits = RunLimits {
        max_exits: 100,
        trace_capacity: 10,
    };
    let mut bus = DeviceBus::new();
    let outcome = run_vcpu(&mut vcpu, &mut bus, &mut vm, limits).unwrap();
    assert_eq!(outcome.stop, StopReason::ExitLimit);
    assert_eq!(outcome.exits, 100);
    assert_eq!(outcome.trace.len(), 10);
    assert_eq!(vcpu.remaining(), 900, "no exits run past the limit");
    assert_eq!(vm.state(), VmState::Failed);
}

#[test]
fn refuses_to_run_a_vm_that_is_not_running() {
    for state_events in [&[][..], &[VmEvent::Configure][..]] {
        let mut vm = VmLifecycle::new();
        for e in state_events {
            vm.apply(*e).unwrap();
        }
        let before = vm.state();
        let mut vcpu = ScriptedVcpu::new([Step::Exit(VmExit::Hlt)]);
        let err = run_vcpu(
            &mut vcpu,
            &mut DeviceBus::new(),
            &mut vm,
            RunLimits::default(),
        )
        .unwrap_err();
        assert_eq!(err.0, before);
        assert_eq!(vm.state(), before);
        assert_eq!(vcpu.remaining(), 1, "vCPU was not run");
    }
}

/// A device that serves MMIO reads as `0xA5` and records writes.
#[derive(Default)]
struct MmioRegs(Vec<(u64, u64)>);

impl aegis_core::MmioDevice for MmioRegs {
    fn read(&mut self, _offset: u64, _size: AccessSize) -> u64 {
        0xA5
    }
    fn write(&mut self, offset: u64, _size: AccessSize, value: u64) {
        self.0.push((offset, value));
    }
}

#[test]
fn mmio_served_by_a_device_resumes_and_unclaimed_mmio_stops() {
    let mut regs = MmioRegs::default();
    let mut vcpu = ScriptedVcpu::new([
        Step::MmioWrite {
            gpa: 0xD000_0070,
            size: AccessSize::Dword,
            value: 0x0F,
        },
        Step::MmioRead {
            gpa: 0xD000_0000,
            size: AccessSize::Dword,
        },
        Step::MmioWrite {
            gpa: 0xE000_0000,
            size: AccessSize::Byte,
            value: 1,
        },
        Step::Exit(VmExit::Hlt),
    ]);
    let mut vm = running();
    let outcome = {
        let mut bus = DeviceBus::new();
        bus.attach_mmio(0xD000_0000, 0x1000, &mut regs).unwrap();
        run_vcpu(&mut vcpu, &mut bus, &mut vm, RunLimits::default()).unwrap()
    };
    let unclaimed = MmioAccess::new(0xE000_0000, AccessSize::Byte, Some(1)).unwrap();
    assert_eq!(outcome.stop, StopReason::UnhandledMmio(unclaimed));
    assert_eq!(outcome.exits, 3);
    assert!(matches!(outcome.trace[0], VmExit::MmioDevice(_)));
    assert!(matches!(outcome.trace[1], VmExit::MmioDevice(_)));
    assert_eq!(regs.0, [(0x70, 0x0F)]);
    assert_eq!(vcpu.mmio_reads, [0xA5]);
    assert_eq!(vcpu.remaining(), 1, "HLT never reached");
    assert_eq!(vm.state(), VmState::Failed);
}

#[test]
fn stop_flag_set_before_entry_means_no_entry() {
    use std::sync::atomic::AtomicBool;
    let flag = AtomicBool::new(true);
    let mut vcpu = ScriptedVcpu::new([Step::Exit(VmExit::Hlt)]);
    let outcome = run_vcpu_until(
        &mut vcpu,
        &mut DeviceBus::new(),
        RunLimits::default(),
        &flag,
    );
    assert_eq!(outcome.stop, StopReason::Requested);
    assert_eq!(outcome.exits, 0);
    assert_eq!(vcpu.remaining(), 1);
}

/// A flag that is clear when the vCPU is about to enter the guest and set
/// afterwards: another vCPU stopped the VM while this one was inside the
/// backend.
struct SetAfterEntry(std::cell::Cell<bool>);

impl aegis_core::StopFlag for SetAfterEntry {
    fn is_set(&self) -> bool {
        self.0.replace(true)
    }
}

#[test]
fn host_interrupt_is_requested_only_when_the_flag_is_set() {
    let mut kicked = ScriptedVcpu::new([Step::Interrupted]);
    let flag = SetAfterEntry(std::cell::Cell::new(false));
    let outcome = run_vcpu_until(
        &mut kicked,
        &mut DeviceBus::new(),
        RunLimits::default(),
        &flag,
    );
    assert_eq!(
        outcome.stop,
        StopReason::Requested,
        "kicked out by another vCPU"
    );
    assert_eq!(outcome.exits, 0);

    let mut unexpected = ScriptedVcpu::new([Step::Interrupted]);
    let never = std::sync::atomic::AtomicBool::new(false);
    let outcome = run_vcpu_until(
        &mut unexpected,
        &mut DeviceBus::new(),
        RunLimits::default(),
        &never,
    );
    assert_eq!(
        outcome.stop,
        StopReason::BackendError("mock vCPU interrupted by the host".to_owned()),
        "an interrupt nobody asked for is a backend error"
    );
}
