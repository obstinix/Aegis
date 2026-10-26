//! Backend-neutral device models for Aegis (ADR-0008).
//!
//! Devices implement [`aegis_core::PortDevice`] and are shared by the hosted
//! and bare-metal backends. `no_std` + `alloc`, no `unsafe`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod serial;

pub use serial::Serial16550;
