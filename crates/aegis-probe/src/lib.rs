//! Host capability detection for Aegis (roadmap milestone M1).
//!
//! - [`cpuid`]: decoding of the CPUID leaves Aegis cares about. `no_std`,
//!   usable by the bare-metal core.
//! - [`linux`] (feature `std`): parsers for `/proc` and `/sys` sources,
//!   reading from a configurable root so tests can use captured fixtures.
//! - [`kvm`] (feature `std`): the state of `/dev/kvm`; live probing on Linux.
//! - [`report`] (feature `std`): turns [`report::Observations`] into the
//!   `aegis doctor` report and its verdicts. Evaluation is a pure function,
//!   so the same code runs on live observations and on fixtures.
//!
//! Collection and evaluation are kept apart on purpose: everything that
//! decides what Aegis tells the user is testable without the hardware.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

pub mod cpuid;

#[cfg(feature = "std")]
pub mod kvm;
#[cfg(feature = "std")]
pub mod linux;
#[cfg(feature = "std")]
pub mod report;
