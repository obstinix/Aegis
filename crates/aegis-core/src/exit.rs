//! Backend-neutral VM-exit model (ADR-0007).
//!
//! Each backend translates its native exit information (a `kvm_run` exit
//! reason, or a VMCS exit reason plus qualification) into a [`VmExit`]. The
//! types here validate what they can at construction, so malformed exit data
//! is rejected before any device model sees it.

use core::fmt;

/// Width of a port or MMIO access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessSize {
    /// 1 byte.
    Byte = 1,
    /// 2 bytes.
    Word = 2,
    /// 4 bytes.
    Dword = 4,
    /// 8 bytes. Not valid for port I/O.
    Qword = 8,
}

impl AccessSize {
    /// Width in bytes.
    #[must_use]
    pub const fn bytes(self) -> u8 {
        self as u8
    }

    /// Mask selecting the low `bytes()` bytes of a value.
    #[must_use]
    pub const fn mask(self) -> u64 {
        match self {
            Self::Byte => 0xFF,
            Self::Word => 0xFFFF,
            Self::Dword => 0xFFFF_FFFF,
            Self::Qword => u64::MAX,
        }
    }
}

/// An access width that is not 1, 2, 4 or 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAccessSize(pub u8);

impl fmt::Display for InvalidAccessSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid access size {} (expected 1, 2, 4 or 8)", self.0)
    }
}

impl core::error::Error for InvalidAccessSize {}

impl TryFrom<u8> for AccessSize {
    type Error = InvalidAccessSize;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Byte),
            2 => Ok(Self::Word),
            4 => Ok(Self::Dword),
            8 => Ok(Self::Qword),
            other => Err(InvalidAccessSize(other)),
        }
    }
}

/// A port I/O access by the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoAccess {
    port: u16,
    size: AccessSize,
    write: Option<u32>,
}

impl IoAccess {
    /// A port I/O access. `write` is the value written (masked to `size`),
    /// or `None` for a read.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAccessSize`] for 8-byte accesses, which x86 port I/O
    /// cannot perform.
    pub fn new(port: u16, size: AccessSize, write: Option<u32>) -> Result<Self, InvalidAccessSize> {
        if size == AccessSize::Qword {
            return Err(InvalidAccessSize(size.bytes()));
        }
        // Masking only drops bits, and a Dword mask fits in u32.
        let mask = u32::try_from(size.mask()).unwrap_or(u32::MAX);
        Ok(Self {
            port,
            size,
            write: write.map(|v| v & mask),
        })
    }

    /// Port number.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Access width.
    #[must_use]
    pub const fn size(&self) -> AccessSize {
        self.size
    }

    /// Value written, already masked to the access width, or `None` for a read.
    #[must_use]
    pub const fn write_value(&self) -> Option<u32> {
        self.write
    }
}

/// A memory-mapped I/O access by the guest to an address with no RAM behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioAccess {
    gpa: u64,
    size: AccessSize,
    write: Option<u64>,
}

impl MmioAccess {
    /// An MMIO access. `write` is the value written (masked to `size`), or
    /// `None` for a read.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAccessSize`] if the access's last byte would lie past
    /// `u64::MAX`. An access ending exactly at `u64::MAX` is valid.
    pub fn new(gpa: u64, size: AccessSize, write: Option<u64>) -> Result<Self, InvalidAccessSize> {
        // Check the inclusive last byte: `gpa + bytes` (the exclusive end) is
        // 2^64 for an access touching the top byte and would not fit in a u64.
        let last_byte_offset = u64::from(size.bytes()).saturating_sub(1);
        if gpa.checked_add(last_byte_offset).is_none() {
            return Err(InvalidAccessSize(size.bytes()));
        }
        Ok(Self {
            gpa,
            size,
            write: write.map(|v| v & size.mask()),
        })
    }

    /// Guest-physical address.
    #[must_use]
    pub const fn gpa(&self) -> u64 {
        self.gpa
    }

    /// Access width.
    #[must_use]
    pub const fn size(&self) -> AccessSize {
        self.size
    }

    /// Value written, already masked to the access width, or `None` for a read.
    #[must_use]
    pub const fn write_value(&self) -> Option<u64> {
        self.write
    }
}

/// Why a vCPU stopped executing guest code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmExit {
    /// Port I/O.
    Io(IoAccess),
    /// MMIO to a guest-physical address with neither RAM nor a device.
    Mmio(MmioAccess),
    /// MMIO served by a device (e.g. a virtio-mmio transport).
    MmioDevice(MmioAccess),
    /// The guest executed `HLT`.
    Hlt,
    /// The guest requested shutdown, or hit a triple fault.
    Shutdown,
    /// The guest executed `CPUID` (bare-metal backend only; KVM handles CPUID in the kernel).
    Cpuid {
        /// `EAX` input.
        leaf: u32,
        /// `ECX` input.
        subleaf: u32,
    },
    /// Any exit the backend has no mapping for. Dispatch must stop the VM
    /// with diagnostics, never resume it.
    Unhandled {
        /// Backend-specific raw exit reason, for diagnostics.
        raw_reason: u64,
    },
}

impl VmExit {
    /// Whether the guest can be resumed after this exit once it has been handled.
    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::MmioDevice(_) | Self::Cpuid { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_size_round_trips_and_rejects_others() {
        for size in [
            AccessSize::Byte,
            AccessSize::Word,
            AccessSize::Dword,
            AccessSize::Qword,
        ] {
            assert_eq!(AccessSize::try_from(size.bytes()), Ok(size));
        }
        for bad in [0u8, 3, 5, 6, 7, 9, 16, 255] {
            assert_eq!(AccessSize::try_from(bad), Err(InvalidAccessSize(bad)));
        }
    }

    #[test]
    fn io_write_value_is_masked_to_width() {
        let io = IoAccess::new(0x3F8, AccessSize::Byte, Some(0xDEAD_BE41)).unwrap();
        assert_eq!(io.write_value(), Some(0x41));
        let io = IoAccess::new(0x3F8, AccessSize::Word, Some(0xDEAD_BEEF)).unwrap();
        assert_eq!(io.write_value(), Some(0xBEEF));
        let io = IoAccess::new(0x3F8, AccessSize::Dword, Some(u32::MAX)).unwrap();
        assert_eq!(io.write_value(), Some(u32::MAX));
        assert_eq!(
            IoAccess::new(0x3F8, AccessSize::Byte, None)
                .unwrap()
                .write_value(),
            None
        );
    }

    #[test]
    fn io_rejects_qword() {
        assert_eq!(
            IoAccess::new(0x80, AccessSize::Qword, None),
            Err(InvalidAccessSize(8))
        );
    }

    #[test]
    fn mmio_masks_and_rejects_wrapping() {
        let m = MmioAccess::new(0xFEE0_0000, AccessSize::Dword, Some(u64::MAX)).unwrap();
        assert_eq!(m.write_value(), Some(0xFFFF_FFFF));
        assert!(MmioAccess::new(u64::MAX - 7, AccessSize::Qword, None).is_ok());
        assert!(MmioAccess::new(u64::MAX - 6, AccessSize::Qword, None).is_err());
    }

    #[test]
    fn only_handled_exits_are_resumable() {
        let io = VmExit::Io(IoAccess::new(0x3F8, AccessSize::Byte, Some(0)).unwrap());
        assert!(io.is_resumable());
        assert!(
            VmExit::Cpuid {
                leaf: 0,
                subleaf: 0
            }
            .is_resumable()
        );
        assert!(!VmExit::Hlt.is_resumable());
        assert!(!VmExit::Shutdown.is_resumable());
        assert!(!VmExit::Unhandled { raw_reason: 0xFFFF }.is_resumable());
        let mmio = MmioAccess::new(0xD000_0000, AccessSize::Dword, None).unwrap();
        assert!(
            VmExit::MmioDevice(mmio).is_resumable(),
            "served by a device"
        );
        assert!(
            !VmExit::Mmio(mmio).is_resumable(),
            "no device: the VM stops"
        );
    }
}
