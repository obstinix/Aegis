//! The UART behind the port bus, driven by the mock backend: the same path a
//! KVM guest takes.

use aegis_core::mock::{ScriptedVcpu, Step};
use aegis_core::{
    AccessSize, DeviceBus, RunLimits, StopReason, VmEvent, VmExit, VmLifecycle, run_vcpu,
};
use aegis_devices::Serial16550;

#[test]
fn guest_polling_lsr_then_writing_thr_reaches_the_output() {
    let mut steps = Vec::new();
    for &b in b"Aegis\n" {
        steps.push(Step::In {
            port: 0x3FD,
            size: AccessSize::Byte,
        });
        steps.push(Step::Out {
            port: 0x3F8,
            size: AccessSize::Byte,
            value: u32::from(b),
        });
    }
    steps.push(Step::Exit(VmExit::Hlt));
    let mut vcpu = ScriptedVcpu::new(steps);

    let mut uart = Serial16550::default();
    let mut vm = VmLifecycle::new();
    vm.apply(VmEvent::Configure).unwrap();
    vm.apply(VmEvent::Start).unwrap();
    let outcome = {
        let mut bus = DeviceBus::new();
        bus.attach_port(0x3F8, Serial16550::PORTS, &mut uart)
            .unwrap();
        run_vcpu(&mut vcpu, &mut bus, &mut vm, RunLimits::default()).unwrap()
    };

    assert_eq!(outcome.stop, StopReason::Halted);
    assert_eq!(outcome.exits, 13);
    assert_eq!(uart.output(), b"Aegis\n");
    assert!(
        vcpu.reads.iter().all(|&lsr| lsr & 0x20 != 0),
        "THR always reported empty"
    );
}
