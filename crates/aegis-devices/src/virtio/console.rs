//! virtio-console (virtio 1.2, §5.3), device ID 3: one port, no multiport.
//!
//! - Queue 0, `receiveq`: host-to-guest. Aegis has no input source yet, so
//!   receive buffers are never filled.
//! - Queue 1, `transmitq`: guest-to-host. Device-readable buffers are
//!   appended to the output, which is capped at
//!   [`VirtioConsole::DEFAULT_CAPACITY`] bytes; excess is counted and
//!   dropped. Every chain is returned to the used ring with length 0.
//!
//! No feature bits are offered (no `VIRTIO_CONSOLE_F_SIZE`, `F_MULTIPORT` or
//! `F_EMERG_WRITE`), so the configuration space reads as zero.

use alloc::vec;
use alloc::vec::Vec;

use aegis_core::{AccessSize, GuestMemoryAccess};

use super::VirtioDevice;
use super::queue::{QueueError, SplitQueue};

/// Virtio device ID for console.
pub const DEVICE_ID: u32 = 3;
const RECEIVEQ: usize = 0;
const TRANSMITQ: usize = 1;
const QUEUE_SIZE: u16 = 256;

/// A virtio console whose guest output is collected in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtioConsole {
    output: Vec<u8>,
    capacity: usize,
    dropped: u64,
}

impl Default for VirtioConsole {
    fn default() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }
}

impl VirtioConsole {
    /// Default output cap in bytes.
    pub const DEFAULT_CAPACITY: usize = 64 * 1024;

    /// A console keeping at most `capacity` output bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            output: Vec::new(),
            capacity,
            dropped: 0,
        }
    }

    /// Bytes the guest transmitted (up to the cap).
    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Bytes dropped because the cap was reached.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    fn append(&mut self, data: &[u8]) {
        let room = self.capacity.saturating_sub(self.output.len());
        let keep = data.len().min(room);
        self.output.extend_from_slice(&data[..keep]);
        let lost = u64::try_from(data.len().saturating_sub(keep)).unwrap_or(u64::MAX);
        self.dropped = self.dropped.saturating_add(lost);
    }
}

impl VirtioDevice for VirtioConsole {
    fn device_id(&self) -> u32 {
        DEVICE_ID
    }

    fn device_features(&self) -> u64 {
        0
    }

    fn queue_max_sizes(&self) -> &[u16] {
        &[QUEUE_SIZE, QUEUE_SIZE]
    }

    fn read_config(&self, _offset: u64, _size: AccessSize) -> u64 {
        0
    }

    fn notify(
        &mut self,
        index: usize,
        queue: &mut SplitQueue,
        mem: &dyn GuestMemoryAccess,
    ) -> Result<bool, QueueError> {
        if index == RECEIVEQ {
            // Receive buffers stay with the device until there is input.
            return Ok(false);
        }
        if index != TRANSMITQ {
            return Ok(false);
        }
        let mut returned = false;
        while let Some(chain) = queue.pop(mem)? {
            for buffer in chain.buffers.iter().filter(|b| !b.writable) {
                // Chain length is capped by the queue (MAX_CHAIN_BYTES).
                let len = usize::try_from(buffer.len).map_err(|_| QueueError::ChainTooLarge)?;
                let mut data = vec![0u8; len];
                mem.read(buffer.addr, &mut data)?;
                self.append(&data);
            }
            queue.push_used(mem, chain.head, 0)?;
            returned = true;
        }
        Ok(returned)
    }

    fn reset(&mut self) {
        // Output already delivered to the host stays; only device state resets.
    }
}
