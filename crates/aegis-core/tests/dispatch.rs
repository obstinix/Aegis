//! `run_vcpu` driven by the scripted mock backend: exit handling, lifecycle
//! transitions, limits, and port routing.

use aegis_core::mock::{ScriptedVcpu, Step, io_exit};
use aegis_core::{
    AccessSize, MmioAccess, PortBus, PortDevice, RunLimits, StopReason, VmEvent, VmExit,
    VmLifecycle, VmState, run_vcpu,
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
        let mut bus = PortBus::new();
        bus.attach(0x3F8, 8, &mut sink).unwrap();
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
        &mut PortBus::new(),
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
    let mut bus = PortBus::new();
    bus.attach(0x3F8, 8, &mut sink).unwrap();
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
        &mut PortBus::new(),
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
        &mut PortBus::new(),
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
        &mut PortBus::new(),
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
    let mut bus = PortBus::new();
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
            &mut PortBus::new(),
            &mut vm,
            RunLimits::default(),
        )
        .unwrap_err();
        assert_eq!(err.0, before);
        assert_eq!(vm.state(), before);
        assert_eq!(vcpu.remaining(), 1, "vCPU was not run");
    }
}
