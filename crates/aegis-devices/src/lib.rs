//! Backend-neutral device models for Aegis (ADR-0008): a 16550 UART and
//! virtio-console over virtio-mmio.
//!
//! Devices implement [`aegis_core::PortDevice`] or [`aegis_core::MmioDevice`]
//! and are shared by the hosted and bare-metal backends. `no_std` + `alloc`, no `unsafe`.
//!
//! Console output goes to an [`aegis_core::OutputSink`]: by default a
//! capped in-memory buffer ([`CappedBuffer`]), or a host stream.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod output;
pub mod serial;
pub mod virtio;

pub use output::CappedBuffer;
pub use serial::Serial16550;
pub use virtio::{VirtioConsole, VirtioMmio};
