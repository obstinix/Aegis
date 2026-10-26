//! A scripted mock backend (feature `mock`), for testing dispatch, devices
//! and lifecycle handling without hardware (ADR-0010).

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt;

use crate::backend::{PortIo, Vcpu};
use crate::exit::{AccessSize, IoAccess, VmExit};

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
    /// Any other exit, returned as-is.
    Exit(VmExit),
}

/// The mock ran out of script, or was given an impossible step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// No steps left: a real guest would keep running, so the test script is incomplete.
    ScriptExhausted,
    /// A port step used an 8-byte access.
    InvalidAccess,
}

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptExhausted => write!(f, "mock vCPU script exhausted"),
            Self::InvalidAccess => write!(f, "mock vCPU step uses an invalid port access size"),
        }
    }
}

/// A vCPU that replays a script of guest actions.
#[derive(Debug, Clone, Default)]
pub struct ScriptedVcpu {
    steps: VecDeque<Step>,
    /// Values served to the guest's port reads, in order.
    pub reads: Vec<u32>,
}

impl ScriptedVcpu {
    /// A vCPU that will perform `steps` in order.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            reads: Vec::new(),
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

    fn run(&mut self, io: &mut dyn PortIo) -> Result<VmExit, MockError> {
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
