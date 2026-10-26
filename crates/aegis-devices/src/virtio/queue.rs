//! Split virtqueue (virtio 1.2, §2.7), device side.
//!
//! Everything in the rings is written by the guest driver and is treated as
//! untrusted input:
//! - indices are range-checked;
//! - descriptor chains are bounded by the queue size, which also catches
//!   cycles;
//! - indirect descriptors are rejected (`VIRTIO_F_INDIRECT_DESC` is never
//!   offered);
//! - a chain's total length is capped;
//! - every guest-memory access goes through [`GuestMemoryAccess`], which
//!   rejects ranges outside guest RAM.
//!
//! A violation is a [`QueueError`]. The transport then marks the device
//! `DEVICE_NEEDS_RESET` instead of guessing what the driver meant.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{Ordering, fence};

use aegis_core::{GuestAccessError, GuestMemoryAccess};

/// Descriptor flag: the chain continues at `next`.
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
/// Descriptor flag: the buffer is device-writable (otherwise device-readable).
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
/// Descriptor flag: the buffer holds an indirect descriptor table.
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;
/// Avail-ring flag: the driver does not want a used-buffer interrupt.
pub const VIRTQ_AVAIL_F_NO_INTERRUPT: u16 = 1;

/// Largest total length of one descriptor chain the device accepts.
pub const MAX_CHAIN_BYTES: u64 = 1024 * 1024;

const DESC_SIZE: u64 = 16;
const USED_ELEM_SIZE: u64 = 8;

/// Why a queue could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// Queue size is zero, not a power of two, or above the device maximum.
    BadSize(u16),
    /// A ring address does not meet the spec's alignment.
    Misaligned {
        /// Which ring: `"descriptor"`, `"available"` or `"used"`.
        ring: &'static str,
        /// The address.
        addr: u64,
    },
    /// A ring or buffer lies outside guest memory.
    Memory(GuestAccessError),
    /// An address computation overflowed.
    AddressOverflow,
    /// The driver advanced the available index by more than the queue size.
    AvailOverrun {
        /// Last index the device processed.
        last: u16,
        /// Index the driver published.
        published: u16,
    },
    /// A descriptor index is not below the queue size.
    BadIndex(u16),
    /// A chain is longer than the queue (necessarily a cycle).
    ChainTooLong,
    /// An indirect descriptor, a feature that was not negotiated.
    Indirect,
    /// A chain's buffers total more than [`MAX_CHAIN_BYTES`].
    ChainTooLarge,
    /// A chain the device must write into (e.g. a console receive buffer)
    /// holds a device-readable buffer.
    NotWritable,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadSize(n) => write!(f, "invalid queue size {n}"),
            Self::Misaligned { ring, addr } => write!(f, "{ring} ring at {addr:#x} is misaligned"),
            Self::Memory(e) => write!(f, "queue memory: {e}"),
            Self::AddressOverflow => write!(f, "queue address overflows"),
            Self::AvailOverrun { last, published } => {
                write!(f, "available index jumped from {last} to {published}")
            }
            Self::BadIndex(i) => write!(f, "descriptor index {i} out of range"),
            Self::ChainTooLong => write!(f, "descriptor chain longer than the queue (cycle)"),
            Self::Indirect => write!(f, "indirect descriptor without VIRTIO_F_INDIRECT_DESC"),
            Self::ChainTooLarge => {
                write!(f, "descriptor chain larger than {MAX_CHAIN_BYTES} bytes")
            }
            Self::NotWritable => write!(f, "receive chain holds a device-readable buffer"),
        }
    }
}

impl core::error::Error for QueueError {}

impl From<GuestAccessError> for QueueError {
    fn from(e: GuestAccessError) -> Self {
        Self::Memory(e)
    }
}

/// One buffer of a descriptor chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    /// Guest-physical address.
    pub addr: u64,
    /// Length in bytes.
    pub len: u32,
    /// Device-writable (`true`) or device-readable (`false`).
    pub writable: bool,
}

/// A descriptor chain taken from the available ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    /// Index of the head descriptor; returned in the used ring.
    pub head: u16,
    /// The chain's buffers, in order.
    pub buffers: Vec<Buffer>,
}

fn at(base: u64, offset: u64) -> Result<u64, QueueError> {
    base.checked_add(offset).ok_or(QueueError::AddressOverflow)
}

fn times(index: u64, stride: u64) -> Result<u64, QueueError> {
    index.checked_mul(stride).ok_or(QueueError::AddressOverflow)
}

fn read_u16(mem: &dyn GuestMemoryAccess, gpa: u64) -> Result<u16, QueueError> {
    let mut b = [0u8; 2];
    mem.read(gpa, &mut b)?;
    Ok(u16::from_le_bytes(b))
}

/// Device-side state of one split virtqueue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitQueue {
    /// Size the driver configured (`QueueNum`).
    pub size: u16,
    /// Descriptor table address (`QueueDesc`).
    pub desc: u64,
    /// Available ring address (`QueueDriver`).
    pub avail: u64,
    /// Used ring address (`QueueDevice`).
    pub used: u64,
    ready: bool,
    next_avail: u16,
    next_used: u16,
}

impl SplitQueue {
    /// Whether the queue was enabled with [`SplitQueue::enable`].
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.ready
    }

    /// Check the configuration and enable the queue.
    ///
    /// # Errors
    ///
    /// [`QueueError::BadSize`] or [`QueueError::Misaligned`]; the queue stays
    /// disabled.
    pub fn enable(&mut self, max_size: u16) -> Result<(), QueueError> {
        if self.size == 0 || self.size > max_size || !self.size.is_power_of_two() {
            return Err(QueueError::BadSize(self.size));
        }
        for (ring, addr, align) in [
            ("descriptor", self.desc, 16),
            ("available", self.avail, 2),
            ("used", self.used, 4),
        ] {
            if !addr.is_multiple_of(align) {
                return Err(QueueError::Misaligned { ring, addr });
            }
        }
        self.ready = true;
        self.next_avail = 0;
        self.next_used = 0;
        Ok(())
    }

    /// Disable the queue and forget its configuration (device reset).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Disable the queue (`QueueReady` = 0) but keep the configured size and
    /// addresses, so the driver can enable it again.
    pub fn disable(&mut self) {
        self.ready = false;
        self.next_avail = 0;
        self.next_used = 0;
    }

    fn slot(&self, index: u16) -> u64 {
        // size is a non-zero power of two once ready.
        u64::from(index & self.size.wrapping_sub(1))
    }

    /// Take the next available descriptor chain, if the driver published one.
    ///
    /// # Errors
    ///
    /// [`QueueError`] for any malformed ring, index, descriptor or buffer.
    pub fn pop(&mut self, mem: &dyn GuestMemoryAccess) -> Result<Option<Chain>, QueueError> {
        if !self.ready {
            return Ok(None);
        }
        let published = read_u16(mem, at(self.avail, 2)?)?;
        let pending = published.wrapping_sub(self.next_avail);
        if pending == 0 {
            return Ok(None);
        }
        if pending > self.size {
            return Err(QueueError::AvailOverrun {
                last: self.next_avail,
                published,
            });
        }
        // Read the ring entry only after seeing the index that covers it.
        fence(Ordering::Acquire);
        let entry = at(self.avail, at(4, times(self.slot(self.next_avail), 2)?)?)?;
        let head = read_u16(mem, entry)?;
        let buffers = self.walk(mem, head)?;
        self.next_avail = self.next_avail.wrapping_add(1);
        Ok(Some(Chain { head, buffers }))
    }

    fn walk(&self, mem: &dyn GuestMemoryAccess, head: u16) -> Result<Vec<Buffer>, QueueError> {
        let mut buffers = Vec::new();
        let mut total: u64 = 0;
        let mut index = head;
        loop {
            if index >= self.size {
                return Err(QueueError::BadIndex(index));
            }
            if buffers.len() >= usize::from(self.size) {
                return Err(QueueError::ChainTooLong);
            }
            let mut raw = [0u8; 16];
            mem.read(
                at(self.desc, times(u64::from(index), DESC_SIZE)?)?,
                &mut raw,
            )?;
            let addr = u64::from_le_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ]);
            let len = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
            let flags = u16::from_le_bytes([raw[12], raw[13]]);
            let next = u16::from_le_bytes([raw[14], raw[15]]);
            if flags & VIRTQ_DESC_F_INDIRECT != 0 {
                return Err(QueueError::Indirect);
            }
            total = total.saturating_add(u64::from(len));
            if total > MAX_CHAIN_BYTES {
                return Err(QueueError::ChainTooLarge);
            }
            // Reject buffers whose end wraps the address space. Whether the
            // buffer is guest RAM is checked when the device accesses it,
            // through `GuestMemoryAccess`.
            at(addr, u64::from(len))?;
            buffers.push(Buffer {
                addr,
                len,
                writable: flags & VIRTQ_DESC_F_WRITE != 0,
            });
            if flags & VIRTQ_DESC_F_NEXT == 0 {
                return Ok(buffers);
            }
            index = next;
        }
    }

    /// Return chain `head` to the driver, with `written` bytes written into it.
    ///
    /// # Errors
    ///
    /// [`QueueError::Memory`] if the used ring is outside guest memory.
    pub fn push_used(
        &mut self,
        mem: &dyn GuestMemoryAccess,
        head: u16,
        written: u32,
    ) -> Result<(), QueueError> {
        let entry = at(
            self.used,
            at(4, times(self.slot(self.next_used), USED_ELEM_SIZE)?)?,
        )?;
        let mut elem = [0u8; 8];
        elem[..4].copy_from_slice(&u32::from(head).to_le_bytes());
        elem[4..].copy_from_slice(&written.to_le_bytes());
        mem.write(entry, &elem)?;
        self.next_used = self.next_used.wrapping_add(1);
        // The entry must be visible before the index that publishes it.
        fence(Ordering::Release);
        mem.write(at(self.used, 2)?, &self.next_used.to_le_bytes())?;
        Ok(())
    }

    /// Whether the driver wants an interrupt for used buffers.
    ///
    /// # Errors
    ///
    /// [`QueueError::Memory`] if the available ring is outside guest memory.
    pub fn wants_interrupt(&self, mem: &dyn GuestMemoryAccess) -> Result<bool, QueueError> {
        Ok(read_u16(mem, self.avail)? & VIRTQ_AVAIL_F_NO_INTERRUPT == 0)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // test layouts use small fixed offsets
mod tests {
    use super::*;
    use aegis_core::mock::VecMemory;

    const DESC: u64 = 0x1000;
    const AVAIL: u64 = 0x2000;
    const USED: u64 = 0x3000;

    fn queue(size: u16) -> SplitQueue {
        let mut q = SplitQueue {
            size,
            desc: DESC,
            avail: AVAIL,
            used: USED,
            ..SplitQueue::default()
        };
        q.enable(256).unwrap();
        q
    }

    fn desc(mem: &VecMemory, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let mut raw = [0u8; 16];
        raw[..8].copy_from_slice(&addr.to_le_bytes());
        raw[8..12].copy_from_slice(&len.to_le_bytes());
        raw[12..14].copy_from_slice(&flags.to_le_bytes());
        raw[14..].copy_from_slice(&next.to_le_bytes());
        mem.write(DESC + u64::from(i) * 16, &raw).unwrap();
    }

    fn publish(mem: &VecMemory, slot: u16, head: u16, idx: u16) {
        mem.write(AVAIL + 4 + u64::from(slot) * 2, &head.to_le_bytes())
            .unwrap();
        mem.write(AVAIL + 2, &idx.to_le_bytes()).unwrap();
    }

    #[test]
    fn enable_validates_size_and_alignment() {
        let base = SplitQueue {
            desc: DESC,
            avail: AVAIL,
            used: USED,
            ..SplitQueue::default()
        };
        for bad in [0u16, 3, 6, 512] {
            let mut q = SplitQueue {
                size: bad,
                ..base.clone()
            };
            assert_eq!(q.enable(256), Err(QueueError::BadSize(bad)));
            assert!(!q.is_ready());
        }
        let mut q = SplitQueue {
            size: 8,
            desc: DESC + 8,
            ..base.clone()
        };
        assert!(matches!(
            q.enable(256),
            Err(QueueError::Misaligned {
                ring: "descriptor",
                ..
            })
        ));
        let mut q = SplitQueue {
            size: 8,
            avail: AVAIL + 1,
            ..base.clone()
        };
        assert!(matches!(
            q.enable(256),
            Err(QueueError::Misaligned {
                ring: "available",
                ..
            })
        ));
        let mut q = SplitQueue {
            size: 8,
            used: USED + 2,
            ..base.clone()
        };
        assert!(matches!(
            q.enable(256),
            Err(QueueError::Misaligned { ring: "used", .. })
        ));
        let mut q = SplitQueue { size: 256, ..base };
        assert!(q.enable(256).is_ok() && q.is_ready());
    }

    #[test]
    fn pops_single_and_chained_buffers_then_returns_them() {
        let mem = VecMemory::new(0x10000);
        let mut q = queue(8);
        assert_eq!(q.pop(&mem).unwrap(), None, "nothing published");

        desc(&mem, 3, 0x5000, 5, 0, 0);
        publish(&mem, 0, 3, 1);
        let chain = q.pop(&mem).unwrap().unwrap();
        assert_eq!(
            chain,
            Chain {
                head: 3,
                buffers: vec![Buffer {
                    addr: 0x5000,
                    len: 5,
                    writable: false
                }]
            }
        );
        assert_eq!(q.pop(&mem).unwrap(), None);

        desc(&mem, 0, 0x6000, 2, VIRTQ_DESC_F_NEXT, 1);
        desc(&mem, 1, 0x7000, 4, VIRTQ_DESC_F_WRITE, 0);
        publish(&mem, 1, 0, 2);
        let chain = q.pop(&mem).unwrap().unwrap();
        assert_eq!(chain.head, 0);
        assert_eq!(chain.buffers.len(), 2);
        assert!(chain.buffers[1].writable);

        q.push_used(&mem, 3, 0).unwrap();
        q.push_used(&mem, 0, 4).unwrap();
        let mut b = [0u8; 2];
        mem.read(USED + 2, &mut b).unwrap();
        assert_eq!(u16::from_le_bytes(b), 2, "used index published");
        let mut e = [0u8; 8];
        mem.read(USED + 4 + 8, &mut e).unwrap();
        assert_eq!(
            (
                u32::from_le_bytes(e[..4].try_into().unwrap()),
                u32::from_le_bytes(e[4..].try_into().unwrap())
            ),
            (0, 4)
        );
    }

    #[test]
    fn ring_indices_wrap_at_u16() {
        let mem = VecMemory::new(0x10000);
        let mut q = queue(4);
        q.next_avail = u16::MAX;
        q.next_used = u16::MAX;
        desc(&mem, 2, 0x5000, 1, 0, 0);
        publish(&mem, 3, 2, 0); // slot 0xFFFF & 3 == 3; index wraps to 0
        assert_eq!(q.pop(&mem).unwrap().unwrap().head, 2);
        q.push_used(&mem, 2, 0).unwrap();
        let mut b = [0u8; 2];
        mem.read(USED + 2, &mut b).unwrap();
        assert_eq!(u16::from_le_bytes(b), 0);
    }

    #[test]
    fn rejects_malformed_rings_and_descriptors() {
        let fresh = || (VecMemory::new(0x10000), queue(4));

        let (mem, mut q) = fresh();
        publish(&mem, 0, 0, 5); // 5 pending in a queue of 4
        assert_eq!(
            q.pop(&mem),
            Err(QueueError::AvailOverrun {
                last: 0,
                published: 5
            })
        );

        let (mem, mut q) = fresh();
        publish(&mem, 0, 9, 1); // head out of range
        assert_eq!(q.pop(&mem), Err(QueueError::BadIndex(9)));

        let (mem, mut q) = fresh();
        desc(&mem, 0, 0x5000, 1, VIRTQ_DESC_F_NEXT, 7); // next out of range
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem), Err(QueueError::BadIndex(7)));

        let (mem, mut q) = fresh();
        desc(&mem, 0, 0x5000, 1, VIRTQ_DESC_F_NEXT, 1);
        desc(&mem, 1, 0x5000, 1, VIRTQ_DESC_F_NEXT, 0); // 0 -> 1 -> 0 ...
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem), Err(QueueError::ChainTooLong));

        let (mem, mut q) = fresh();
        desc(&mem, 0, 0x5000, 16, VIRTQ_DESC_F_INDIRECT, 0);
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem), Err(QueueError::Indirect));

        let (mem, mut q) = fresh();
        desc(&mem, 0, 0x5000, u32::MAX, 0, 0);
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem), Err(QueueError::ChainTooLarge));

        let (mem, mut q) = fresh();
        desc(&mem, 0, u64::MAX - 1, 8, 0, 0);
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem), Err(QueueError::AddressOverflow));

        let mem = VecMemory::new(0x2000); // avail ring at 0x2000 is outside
        let mut q = queue(4);
        assert!(matches!(q.pop(&mem), Err(QueueError::Memory(_))));
    }

    #[test]
    fn disabled_queue_yields_nothing_and_reset_forgets_everything() {
        let mem = VecMemory::new(0x10000);
        let mut q = SplitQueue {
            size: 4,
            desc: DESC,
            avail: AVAIL,
            used: USED,
            ..SplitQueue::default()
        };
        publish(&mem, 0, 0, 1);
        assert_eq!(q.pop(&mem).unwrap(), None, "not enabled");
        q.enable(256).unwrap();
        q.reset();
        assert_eq!(q, SplitQueue::default());
    }

    #[test]
    fn no_interrupt_flag_is_honoured() {
        let mem = VecMemory::new(0x10000);
        let q = queue(4);
        assert!(q.wants_interrupt(&mem).unwrap());
        mem.write(AVAIL, &VIRTQ_AVAIL_F_NO_INTERRUPT.to_le_bytes())
            .unwrap();
        assert!(!q.wants_interrupt(&mem).unwrap());
    }
}
