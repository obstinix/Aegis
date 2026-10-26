//! Virtio devices over the virtio-mmio transport (virtio 1.2, OASIS).
//!
//! - [`queue`]: the split virtqueue, with all driver-controlled data validated.
//! - [`mmio`]: the virtio-mmio v2 register interface ([`VirtioMmio`]), which
//!   wraps any [`VirtioDevice`] into an [`aegis_core::MmioDevice`].
//! - [`console`]: virtio-console ([`VirtioConsole`]), device ID 3.
//!
//! Devices reach guest RAM only through [`aegis_core::GuestMemoryAccess`], and
//! signal the guest only through an [`aegis_core::InterruptLine`].

pub mod console;
pub mod mmio;
pub mod queue;

pub use console::VirtioConsole;
pub use mmio::VirtioMmio;
pub use queue::{QueueError, SplitQueue};

use aegis_core::{AccessSize, GuestMemoryAccess};

/// `VIRTIO_F_VERSION_1`: the device follows virtio 1.x (required for
/// virtio-mmio version 2). The transport always offers it.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// A virtio device behind a transport.
pub trait VirtioDevice {
    /// Virtio device ID (e.g. 3 for console).
    fn device_id(&self) -> u32;
    /// Device-specific feature bits offered, excluding transport features
    /// such as [`VIRTIO_F_VERSION_1`].
    fn device_features(&self) -> u64;
    /// Maximum size of each queue; the slice length is the number of queues.
    fn queue_max_sizes(&self) -> &[u16];
    /// Read `size` bytes of device configuration space at `offset`.
    fn read_config(&self, offset: u64, size: AccessSize) -> u64;
    /// The driver notified queue `index`: process its available buffers.
    /// Returns whether any buffer was returned to the used ring.
    ///
    /// # Errors
    ///
    /// [`QueueError`] if the driver's queue data is malformed.
    fn notify(
        &mut self,
        index: usize,
        queue: &mut SplitQueue,
        mem: &dyn GuestMemoryAccess,
    ) -> Result<bool, QueueError>;
    /// A queue the device wants processed next, because processing the last
    /// one gave it work there (the console opening its receive path on the
    /// first transmit). The transport asks after each processing and acts on
    /// it in the same exit. The default never asks.
    fn take_requested_queue(&mut self) -> Option<usize> {
        None
    }
    /// Return to the initial state (transport reset).
    fn reset(&mut self);
}
