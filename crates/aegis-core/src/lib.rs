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
//! Status: implemented and unit-tested on the host. No backend uses it yet.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod exit;
pub mod lifecycle;
pub mod memory;

pub use exit::{AccessSize, InvalidAccessSize, IoAccess, MmioAccess, VmExit};
pub use lifecycle::{LifecycleError, VmEvent, VmLifecycle, VmState};
pub use memory::{GuestMemoryMap, MemoryError, MemoryRegion, PAGE_SIZE, Permissions};
