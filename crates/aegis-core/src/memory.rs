//! Validated guest-physical memory map (ADR-0006).
//!
//! A [`GuestMemoryMap`] is the single source of truth for which guest-physical
//! addresses (GPAs) a VM may use. Backends build their second-level
//! translation (KVM memory slots, EPT/NPT) from it, and every GPA range that
//! the hypervisor reads on a guest's behalf is checked against it first.
//!
//! All arithmetic is checked: a region or access whose end would wrap past
//! `u64::MAX` is rejected, never truncated.

use alloc::vec::Vec;
use core::fmt;

/// Granularity of guest memory regions: 4 KiB.
pub const PAGE_SIZE: u64 = 0x1000;

/// Access permissions of a guest memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Permissions {
    /// Guest may read.
    pub read: bool,
    /// Guest may write.
    pub write: bool,
    /// Guest may execute.
    pub execute: bool,
}

impl Permissions {
    /// Read, write and execute: ordinary guest RAM.
    pub const RWX: Self = Self {
        read: true,
        write: true,
        execute: true,
    };
    /// Read and execute only, e.g. a firmware image.
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
    /// Read only.
    pub const R: Self = Self {
        read: true,
        write: false,
        execute: false,
    };

    /// Whether these permissions grant everything `needed` asks for.
    #[must_use]
    pub const fn allows(self, needed: Self) -> bool {
        (self.read || !needed.read)
            && (self.write || !needed.write)
            && (self.execute || !needed.execute)
    }
}

/// One contiguous guest-physical region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    /// First guest-physical address. Page-aligned.
    pub start: u64,
    /// Length in bytes. Non-zero and a multiple of [`PAGE_SIZE`].
    pub size: u64,
    /// What the guest may do with this region.
    pub permissions: Permissions,
}

impl MemoryRegion {
    /// Exclusive end address, or `None` if it would wrap.
    #[must_use]
    pub const fn end(&self) -> Option<u64> {
        self.start.checked_add(self.size)
    }

    /// Whether `[gpa, gpa + len)` lies entirely inside this region.
    /// A zero-length range is never contained.
    #[must_use]
    pub fn contains_range(&self, gpa: u64, len: u64) -> bool {
        let (Some(region_end), Some(access_end)) = (self.end(), gpa.checked_add(len)) else {
            return false;
        };
        len != 0 && gpa >= self.start && access_end <= region_end
    }
}

/// Why a memory map or an access was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The map has no regions.
    Empty,
    /// A region has size zero.
    ZeroSize {
        /// Index of the region in the input.
        index: usize,
    },
    /// A region's start or size is not a multiple of [`PAGE_SIZE`].
    Unaligned {
        /// Index of the region in the input.
        index: usize,
    },
    /// A region's end address would exceed `u64::MAX`.
    Overflow {
        /// Index of the region in the input.
        index: usize,
    },
    /// Two regions share at least one address.
    Overlap {
        /// Start of the lower region.
        first: u64,
        /// Start of the higher region.
        second: u64,
    },
    /// The total size of all regions exceeds the configured cap.
    ExceedsCap {
        /// Total requested bytes.
        total: u64,
        /// Configured maximum in bytes.
        cap: u64,
    },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => write!(f, "guest memory map has no regions"),
            Self::ZeroSize { index } => write!(f, "region {index} has size zero"),
            Self::Unaligned { index } => write!(f, "region {index} is not {PAGE_SIZE:#x}-aligned"),
            Self::Overflow { index } => {
                write!(f, "region {index} extends past the 64-bit address space")
            }
            Self::Overlap { first, second } => {
                write!(f, "regions starting at {first:#x} and {second:#x} overlap")
            }
            Self::ExceedsCap { total, cap } => {
                write!(f, "guest memory total {total:#x} exceeds cap {cap:#x}")
            }
        }
    }
}

impl core::error::Error for MemoryError {}

/// A validated, sorted, non-overlapping set of guest memory regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestMemoryMap {
    regions: Vec<MemoryRegion>,
    total: u64,
}

impl GuestMemoryMap {
    /// Validate `regions` and build a map whose total size is at most
    /// `cap_bytes`. Regions may be given in any order.
    ///
    /// # Errors
    ///
    /// Returns the first [`MemoryError`] found. Per-region checks use the
    /// index in the input slice.
    pub fn new(regions: &[MemoryRegion], cap_bytes: u64) -> Result<Self, MemoryError> {
        if regions.is_empty() {
            return Err(MemoryError::Empty);
        }

        let mut total: u64 = 0;
        for (index, region) in regions.iter().enumerate() {
            if region.size == 0 {
                return Err(MemoryError::ZeroSize { index });
            }
            if !region.start.is_multiple_of(PAGE_SIZE) || !region.size.is_multiple_of(PAGE_SIZE) {
                return Err(MemoryError::Unaligned { index });
            }
            if region.end().is_none() {
                return Err(MemoryError::Overflow { index });
            }
            // Saturating is fine: a saturated total is always above any cap
            // that is itself a u64, so the cap check below rejects it.
            total = total.saturating_add(region.size);
        }
        if total > cap_bytes {
            return Err(MemoryError::ExceedsCap {
                total,
                cap: cap_bytes,
            });
        }

        let mut sorted = regions.to_vec();
        sorted.sort_unstable_by_key(|r| r.start);
        for pair in sorted.windows(2) {
            let (lower, higher) = (pair[0], pair[1]);
            // `end()` was checked above for every region. Should that ever
            // change, treating a missing end as the top of the address space
            // errs towards reporting an overlap, never towards accepting one.
            let lower_end = lower.end().unwrap_or(u64::MAX);
            if higher.start < lower_end {
                return Err(MemoryError::Overlap {
                    first: lower.start,
                    second: higher.start,
                });
            }
        }

        Ok(Self {
            regions: sorted,
            total,
        })
    }

    /// Regions in ascending address order.
    #[must_use]
    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions
    }

    /// Sum of all region sizes in bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total
    }

    /// The region that wholly contains `[gpa, gpa + len)`, if any.
    ///
    /// An access that straddles two adjacent regions returns `None`: callers
    /// must split it, so each part is checked against its own permissions.
    #[must_use]
    pub fn region_for(&self, gpa: u64, len: u64) -> Option<&MemoryRegion> {
        let idx = self
            .regions
            .partition_point(|r| r.start <= gpa)
            .checked_sub(1)?;
        let region = &self.regions[idx];
        region.contains_range(gpa, len).then_some(region)
    }

    /// Whether the guest may perform an access needing `needed` on
    /// `[gpa, gpa + len)`.
    #[must_use]
    pub fn check_access(&self, gpa: u64, len: u64, needed: Permissions) -> bool {
        self.region_for(gpa, len)
            .is_some_and(|r| r.permissions.allows(needed))
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // test fixtures use small, known-safe constants
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn ram(start: u64, size: u64) -> MemoryRegion {
        MemoryRegion {
            start,
            size,
            permissions: Permissions::RWX,
        }
    }

    #[test]
    fn accepts_and_sorts_valid_regions() {
        let map = GuestMemoryMap::new(&[ram(0x10_0000, MIB), ram(0, 0x9_F000)], 16 * MIB).unwrap();
        assert_eq!(map.regions()[0].start, 0);
        assert_eq!(map.regions()[1].start, 0x10_0000);
        assert_eq!(map.total_bytes(), MIB + 0x9_F000);
    }

    #[test]
    fn rejects_empty_map() {
        assert_eq!(GuestMemoryMap::new(&[], MIB), Err(MemoryError::Empty));
    }

    #[test]
    fn rejects_zero_size() {
        assert_eq!(
            GuestMemoryMap::new(&[ram(0, PAGE_SIZE), ram(MIB, 0)], 16 * MIB),
            Err(MemoryError::ZeroSize { index: 1 })
        );
    }

    #[test]
    fn rejects_unaligned_start_and_size() {
        assert_eq!(
            GuestMemoryMap::new(&[ram(0x800, PAGE_SIZE)], MIB),
            Err(MemoryError::Unaligned { index: 0 })
        );
        assert_eq!(
            GuestMemoryMap::new(&[ram(0, 0x1800)], MIB),
            Err(MemoryError::Unaligned { index: 0 })
        );
    }

    #[test]
    fn rejects_region_wrapping_the_address_space() {
        let top = u64::MAX - (PAGE_SIZE - 1); // last page-aligned address
        assert_eq!(
            GuestMemoryMap::new(&[ram(top, 2 * PAGE_SIZE)], u64::MAX),
            Err(MemoryError::Overflow { index: 0 })
        );
    }

    #[test]
    fn rejects_region_ending_at_two_pow_64_but_accepts_the_page_below() {
        // [2^64 - 4 KiB, 2^64) has end 2^64, which does not fit in u64:
        // the map must reject it rather than wrap to 0.
        let top = u64::MAX - (PAGE_SIZE - 1);
        assert!(GuestMemoryMap::new(&[ram(top, PAGE_SIZE)], u64::MAX).is_err());
        // The page below it fits.
        let below = top - PAGE_SIZE;
        assert!(GuestMemoryMap::new(&[ram(below, PAGE_SIZE)], u64::MAX).is_ok());
    }

    #[test]
    fn rejects_overlap_regardless_of_input_order() {
        let err = Err(MemoryError::Overlap {
            first: 0,
            second: MIB,
        });
        assert_eq!(
            GuestMemoryMap::new(&[ram(0, 2 * MIB), ram(MIB, MIB)], 16 * MIB),
            err
        );
        assert_eq!(
            GuestMemoryMap::new(&[ram(MIB, MIB), ram(0, 2 * MIB)], 16 * MIB),
            err
        );
    }

    #[test]
    fn adjacent_regions_do_not_overlap() {
        assert!(GuestMemoryMap::new(&[ram(0, MIB), ram(MIB, MIB)], 16 * MIB).is_ok());
    }

    #[test]
    fn enforces_cap_including_saturation() {
        assert_eq!(
            GuestMemoryMap::new(&[ram(0, 2 * MIB)], MIB),
            Err(MemoryError::ExceedsCap {
                total: 2 * MIB,
                cap: MIB
            })
        );
        // Three huge regions whose sizes sum past u64::MAX: the total
        // saturates instead of wrapping to a small number below the cap.
        let half = (u64::MAX / 2) & !(PAGE_SIZE - 1);
        let result = GuestMemoryMap::new(&[ram(0, half), ram(0, half), ram(0, half)], 16 * MIB);
        assert_eq!(
            result,
            Err(MemoryError::ExceedsCap {
                total: u64::MAX,
                cap: 16 * MIB
            })
        );
    }

    #[test]
    fn region_lookup_respects_bounds() {
        let map = GuestMemoryMap::new(&[ram(0, MIB), ram(4 * MIB, MIB)], 16 * MIB).unwrap();
        assert_eq!(map.region_for(0, 1).unwrap().start, 0);
        assert_eq!(map.region_for(MIB - 8, 8).unwrap().start, 0);
        assert!(map.region_for(MIB - 4, 8).is_none(), "crosses region end");
        assert!(map.region_for(2 * MIB, 1).is_none(), "hole between regions");
        assert_eq!(map.region_for(4 * MIB, PAGE_SIZE).unwrap().start, 4 * MIB);
        assert!(map.region_for(5 * MIB, 1).is_none(), "past last region");
        assert!(map.region_for(0, 0).is_none(), "zero length");
        assert!(map.region_for(u64::MAX, 2).is_none(), "access wraps");
    }

    #[test]
    fn access_straddling_adjacent_regions_is_not_contained() {
        let map = GuestMemoryMap::new(&[ram(0, MIB), ram(MIB, MIB)], 16 * MIB).unwrap();
        assert!(map.region_for(MIB - 4, 8).is_none());
    }

    #[test]
    fn permission_checks() {
        let rom = MemoryRegion {
            start: MIB,
            size: MIB,
            permissions: Permissions::RX,
        };
        let map = GuestMemoryMap::new(&[ram(0, MIB), rom], 16 * MIB).unwrap();
        let write = Permissions {
            read: false,
            write: true,
            execute: false,
        };
        assert!(map.check_access(0, 4, write));
        assert!(!map.check_access(MIB, 4, write), "ROM is not writable");
        assert!(map.check_access(MIB, 4, Permissions::RX));
        assert!(!map.check_access(3 * MIB, 4, Permissions::R), "unmapped");
        assert!(Permissions::RWX.allows(Permissions::RWX));
        assert!(!Permissions::R.allows(Permissions::RX));
    }
}
