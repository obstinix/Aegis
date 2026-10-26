//! Backend-neutral core of the Aegis hypervisor.
//!
//! This crate holds the parts of Aegis that do not depend on how a guest is
//! actually executed: the VM lifecycle state machine, the validated guest
//! physical memory map, and the VM-exit model that both the hosted (KVM) and
//! bare-metal (VMX) backends report into. See ADR-0003, ADR-0006 and ADR-0007
//! in `research/architecture-decision-record.md`.
//!
//! The crate is `no_std` + `alloc` so the bare-metal UEFI core can use it, and
//! it contains no `unsafe` code.
//!
//! It also defines:
//! - the backend interface ([`Vcpu`], [`GuestIo`]);
//! - the device bus for port I/O and MMIO ([`DeviceBus`]), with the traits
//!   devices use to reach guest memory and interrupt lines, and to deliver
//!   console output as it is produced ([`OutputSink`]);
//! - the dispatch loops: [`run_vcpu`] for one vCPU (M2), and
//!   [`run_vcpu_until`] plus [`vm_stop_event`] for SMP guests (M4h).
//!
//! Feature `mock` adds a scripted backend and an in-memory guest RAM for tests.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod backend;
pub mod bus;
pub mod exit;
pub mod lifecycle;
pub mod memory;
#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use backend::{
    GuestIo, MmioIo, NeverStop, NotRunning, PortIo, RunLimits, RunOutcome, StopFlag, StopReason,
    Vcpu, run_vcpu, run_vcpu_until, vm_stop_event,
};
pub use bus::{
    BusError, DeviceBus, GuestAccessError, GuestMemoryAccess, InterruptLine, MmioDevice,
    OutputSink, PortDevice, UnassignedLog,
};

pub use exit::{AccessSize, InvalidAccessSize, IoAccess, MmioAccess, VmExit};
pub use lifecycle::{LifecycleError, VmEvent, VmLifecycle, VmState};
pub use memory::{GuestMemoryMap, MemoryError, MemoryRegion, PAGE_SIZE, Permissions};
