//! Test doubles (feature `mock`), for testing dispatch, devices and lifecycle
//! handling without hardware (ADR-0010):
//! - [`ScriptedVcpu`], a backend that replays guest actions;
//! - [`VecMemory`], bounds-checked guest RAM over a byte vector;
//! - [`RecordingLine`], an interrupt line that remembers its level changes.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use crate::backend::{GuestIo, Vcpu};
use crate::bus::{GuestAccessError, GuestMemoryAccess, InterruptLine};
use crate::exit::{AccessSize, IoAccess, MmioAccess, VmExit};

/// One scripted guest action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The guest writes `value` to `port`.
    Out {
        /// Port.
        port: u16,
        /// Width.
        size: AccessSize,
        /// Value.
        value: u32,
    },
    /// The guest reads from `port`. The value served is recorded in
    /// [`ScriptedVcpu::reads`].
    In {
        /// Port.
        port: u16,
        /// Width.
        size: AccessSize,
    },
    /// The guest writes `value` to MMIO address `gpa`.
    MmioWrite {
        /// Guest-physical address.
        gpa: u64,
        /// Width.
        size: AccessSize,
        /// Value.
        value: u64,
    },
    /// The guest reads MMIO address `gpa`. The value served (if a device
    /// claimed it) is recorded in [`ScriptedVcpu::mmio_reads`].
    MmioRead {
        /// Guest-physical address.
        gpa: u64,
        /// Width.
        size: AccessSize,
    },
    /// The host interrupts the vCPU (as a kick from another vCPU does):
    /// [`Vcpu::run`] returns [`MockError::Interrupted`].
    Interrupted,
    /// Any other exit, returned as-is.
    Exit(VmExit),
}

/// The mock ran out of script, was given an impossible step, or was
/// interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// No steps left: a real guest would keep running, so the test script is incomplete.
    ScriptExhausted,
    /// A step used an access that cannot exist (8-byte port I/O, wrapping MMIO).
    InvalidAccess,
    /// A [`Step::Interrupted`] step.
    Interrupted,
}

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptExhausted => write!(f, "mock vCPU script exhausted"),
            Self::InvalidAccess => write!(f, "mock vCPU step uses an invalid access"),
            Self::Interrupted => write!(f, "mock vCPU interrupted by the host"),
        }
    }
}

/// A vCPU that replays a script of guest actions.
#[derive(Debug, Clone, Default)]
pub struct ScriptedVcpu {
    steps: VecDeque<Step>,
    /// Values served to the guest's port reads, in order.
    pub reads: Vec<u32>,
    /// Values served to the guest's MMIO reads, in order.
    pub mmio_reads: Vec<u64>,
}

impl ScriptedVcpu {
    /// A vCPU that will perform `steps` in order.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            reads: Vec::new(),
            mmio_reads: Vec::new(),
        }
    }

    /// Steps not yet performed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.steps.len()
    }
}

impl Vcpu for ScriptedVcpu {
    type Error = MockError;

    fn run(&mut self, io: &mut dyn GuestIo) -> Result<VmExit, MockError> {
        match self.steps.pop_front().ok_or(MockError::ScriptExhausted)? {
            Step::Out { port, size, value } => {
                let access =
                    IoAccess::new(port, size, Some(value)).map_err(|_| MockError::InvalidAccess)?;
                // Serve the masked value, as hardware would.
                io.write(port, size, access.write_value().unwrap_or(0));
                Ok(VmExit::Io(access))
            }
            Step::In { port, size } => {
                let access =
                    IoAccess::new(port, size, None).map_err(|_| MockError::InvalidAccess)?;
                let mask = u32::try_from(size.mask()).unwrap_or(u32::MAX);
                self.reads.push(io.read(port, size) & mask);
                Ok(VmExit::Io(access))
            }
            Step::MmioWrite { gpa, size, value } => {
                let access = MmioAccess::new(gpa, size, Some(value))
                    .map_err(|_| MockError::InvalidAccess)?;
                if io.mmio_write(gpa, size, access.write_value().unwrap_or(0)) {
                    Ok(VmExit::MmioDevice(access))
                } else {
                    Ok(VmExit::Mmio(access))
                }
            }
            Step::MmioRead { gpa, size } => {
                let access =
                    MmioAccess::new(gpa, size, None).map_err(|_| MockError::InvalidAccess)?;
                match io.mmio_read(gpa, size) {
                    Some(value) => {
                        self.mmio_reads.push(value);
                        Ok(VmExit::MmioDevice(access))
                    }
                    None => Ok(VmExit::Mmio(access)),
                }
            }
            Step::Interrupted => Err(MockError::Interrupted),
            Step::Exit(exit) => Ok(exit),
        }
    }
}

/// A port I/O exit, for building expected traces.
///
/// # Panics
///
/// If `size` is [`AccessSize::Qword`], which port I/O cannot be.
#[must_use]
pub fn io_exit(port: u16, size: AccessSize, write: Option<u32>) -> VmExit {
    VmExit::Io(IoAccess::new(port, size, write).expect("port I/O is at most 4 bytes"))
}

/// Guest RAM `[0, len)` backed by a byte vector. Accesses that do not fit
/// wholly inside it fail; they are never truncated.
#[derive(Debug, Clone)]
pub struct VecMemory {
    bytes: RefCell<Vec<u8>>,
}

impl VecMemory {
    /// `len` bytes of zeroed guest RAM.
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            bytes: RefCell::new(vec![0; len]),
        }
    }

    fn range(&self, gpa: u64, len: usize) -> Result<core::ops::Range<usize>, GuestAccessError> {
        let err = GuestAccessError {
            gpa,
            len: u64::try_from(len).unwrap_or(u64::MAX),
        };
        let start = usize::try_from(gpa).map_err(|_| err)?;
        let end = start.checked_add(len).ok_or(err)?;
        if end > self.bytes.borrow().len() {
            return Err(err);
        }
        Ok(start..end)
    }
}

impl GuestMemoryAccess for VecMemory {
    fn read(&self, gpa: u64, buf: &mut [u8]) -> Result<(), GuestAccessError> {
        let range = self.range(gpa, buf.len())?;
        buf.copy_from_slice(&self.bytes.borrow()[range]);
        Ok(())
    }

    fn write(&self, gpa: u64, data: &[u8]) -> Result<(), GuestAccessError> {
        let range = self.range(gpa, data.len())?;
        self.bytes.borrow_mut()[range].copy_from_slice(data);
        Ok(())
    }
}

/// An interrupt line that records every level it is set to.
#[derive(Debug, Default)]
pub struct RecordingLine {
    levels: RefCell<Vec<bool>>,
}

impl RecordingLine {
    /// Every level set so far, in order.
    #[must_use]
    pub fn levels(&self) -> Vec<bool> {
        self.levels.borrow().clone()
    }

    /// The current level (deasserted if never set).
    #[must_use]
    pub fn asserted(&self) -> bool {
        self.levels.borrow().last().copied().unwrap_or(false)
    }
}

impl InterruptLine for RecordingLine {
    fn set_level(&self, asserted: bool) {
        self.levels.borrow_mut().push(asserted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_memory_rejects_out_of_range_accesses_without_truncating() {
        let mem = VecMemory::new(16);
        mem.write(12, &[1, 2, 3, 4]).unwrap();
        let mut buf = [0u8; 4];
        mem.read(12, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4]);
        assert_eq!(
            mem.write(13, &[0; 4]),
            Err(GuestAccessError { gpa: 13, len: 4 })
        );
        assert!(mem.read(u64::MAX, &mut buf).is_err());
        mem.read(13, &mut [0u8; 3]).unwrap();
        assert!(mem.read(16, &mut [0u8; 1]).is_err());
        mem.read(16, &mut []).unwrap();
    }
}
