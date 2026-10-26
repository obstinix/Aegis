//! Port I/O bus: routes guest port accesses to devices.
//!
//! Devices are borrowed, not owned, so the caller can inspect them (for
//! example, read a serial port's output) after the run.
//!
//! Unassigned ports behave like an empty ISA bus: reads return all ones,
//! writes are dropped. Both are recorded in [`UnassignedLog`] and never stop
//! the guest (roadmap M2 policy).

use alloc::vec::Vec;
use core::fmt;

use crate::backend::PortIo;
use crate::exit::{AccessSize, IoAccess};

/// A device on the port bus. `offset` is relative to the device's base port.
pub trait PortDevice {
    /// A guest read of `size` bytes at `offset`.
    fn read(&mut self, offset: u16, size: AccessSize) -> u32;
    /// A guest write of `value` (masked to `size`) at `offset`.
    fn write(&mut self, offset: u16, size: AccessSize, value: u32);
}

/// Why a device could not be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusError {
    /// `len` was zero.
    EmptyRange,
    /// `base + len` would pass port `0xFFFF`.
    RangeOverflow,
    /// The range overlaps an attached device.
    Overlap {
        /// Base port of the device already attached there.
        existing_base: u16,
    },
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => write!(f, "port range is empty"),
            Self::RangeOverflow => write!(f, "port range extends past 0xFFFF"),
            Self::Overlap { existing_base } => {
                write!(f, "port range overlaps the device at {existing_base:#x}")
            }
        }
    }
}

impl core::error::Error for BusError {}

/// Accesses to ports with no device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnassignedLog {
    /// Total unassigned accesses.
    pub count: u64,
    /// The first [`UnassignedLog::KEEP`] of them, in order. Reads are
    /// recorded with `write_value() == None`.
    pub first: Vec<IoAccess>,
}

impl UnassignedLog {
    /// How many accesses are kept in [`UnassignedLog::first`].
    pub const KEEP: usize = 16;

    fn record(&mut self, port: u16, size: AccessSize, write: Option<u32>) {
        self.count = self.count.saturating_add(1);
        if self.first.len() < Self::KEEP
            && let Ok(access) = IoAccess::new(port, size, write)
        {
            self.first.push(access);
        }
    }
}

struct Attached<'d> {
    base: u16,
    /// Inclusive, so a device can own port 0xFFFF.
    last: u16,
    device: &'d mut dyn PortDevice,
}

/// Routes port I/O to borrowed devices.
#[derive(Default)]
pub struct PortBus<'d> {
    devices: Vec<Attached<'d>>,
    unassigned: UnassignedLog,
}

impl fmt::Debug for PortBus<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortBus")
            .field(
                "ranges",
                &self
                    .devices
                    .iter()
                    .map(|d| (d.base, d.last))
                    .collect::<Vec<_>>(),
            )
            .field("unassigned", &self.unassigned)
            .finish()
    }
}

impl<'d> PortBus<'d> {
    /// An empty bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach `device` at ports `base..base + len`.
    ///
    /// # Errors
    ///
    /// [`BusError`] if the range is empty, passes `0xFFFF`, or overlaps an
    /// attached device.
    pub fn attach(
        &mut self,
        base: u16,
        len: u16,
        device: &'d mut dyn PortDevice,
    ) -> Result<(), BusError> {
        let span = len.checked_sub(1).ok_or(BusError::EmptyRange)?;
        let last = base.checked_add(span).ok_or(BusError::RangeOverflow)?;
        if let Some(existing) = self
            .devices
            .iter()
            .find(|d| base <= d.last && d.base <= last)
        {
            return Err(BusError::Overlap {
                existing_base: existing.base,
            });
        }
        self.devices.push(Attached { base, last, device });
        Ok(())
    }

    /// Accesses that hit no device.
    #[must_use]
    pub fn unassigned(&self) -> &UnassignedLog {
        &self.unassigned
    }

    /// The device covering `port`, and `port`'s offset within it.
    fn route(&mut self, port: u16) -> Option<(&mut (dyn PortDevice + 'd), u16)> {
        self.devices
            .iter_mut()
            .find(|d| d.base <= port && port <= d.last)
            .and_then(|d| Some((&mut *d.device, port.checked_sub(d.base)?)))
    }
}

impl PortIo for PortBus<'_> {
    fn read(&mut self, port: u16, size: AccessSize) -> u32 {
        if let Some((device, offset)) = self.route(port) {
            return device.read(offset, size);
        }
        self.unassigned.record(port, size, None);
        // All ones for the access width; a Dword mask fits in u32.
        u32::try_from(size.mask()).unwrap_or(u32::MAX)
    }

    fn write(&mut self, port: u16, size: AccessSize, value: u32) {
        if let Some((device, offset)) = self.route(port) {
            device.write(offset, size, value);
            return;
        }
        self.unassigned.record(port, size, Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Remembers every access; reads return `0x40 + offset`.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<(u16, u32)>,
        reads: Vec<u16>,
    }

    impl PortDevice for Recorder {
        fn read(&mut self, offset: u16, _size: AccessSize) -> u32 {
            self.reads.push(offset);
            0x40 + u32::from(offset)
        }
        fn write(&mut self, offset: u16, _size: AccessSize, value: u32) {
            self.writes.push((offset, value));
        }
    }

    #[test]
    fn routes_by_offset_within_range() {
        let mut dev = Recorder::default();
        {
            let mut bus = PortBus::new();
            bus.attach(0x3F8, 8, &mut dev).unwrap();
            bus.write(0x3F8, AccessSize::Byte, 0x41);
            bus.write(0x3FF, AccessSize::Byte, 0x7);
            assert_eq!(bus.read(0x3FD, AccessSize::Byte), 0x45);
            assert_eq!(bus.unassigned().count, 0);
        }
        assert_eq!(dev.writes, [(0, 0x41), (7, 0x7)]);
        assert_eq!(dev.reads, [5]);
    }

    #[test]
    fn unassigned_reads_are_all_ones_and_both_directions_are_logged() {
        let mut bus = PortBus::new();
        assert_eq!(bus.read(0x80, AccessSize::Byte), 0xFF);
        assert_eq!(bus.read(0x80, AccessSize::Word), 0xFFFF);
        assert_eq!(bus.read(0x80, AccessSize::Dword), 0xFFFF_FFFF);
        bus.write(0x404, AccessSize::Byte, 0x12);
        let log = bus.unassigned();
        assert_eq!(log.count, 4);
        assert_eq!(log.first[0].write_value(), None);
        assert_eq!(log.first[3].port(), 0x404);
        assert_eq!(log.first[3].write_value(), Some(0x12));
    }

    #[test]
    fn unassigned_log_keeps_only_the_first_accesses() {
        let mut bus = PortBus::new();
        for port in 0..100u16 {
            bus.write(port, AccessSize::Byte, 0);
        }
        assert_eq!(bus.unassigned().count, 100);
        assert_eq!(bus.unassigned().first.len(), UnassignedLog::KEEP);
        assert_eq!(bus.unassigned().first[15].port(), 15);
    }

    #[test]
    fn attach_rejects_bad_ranges() {
        // A device lent to a bus stays borrowed for the bus's lifetime, even
        // if attaching fails, so each attempt gets its own device.
        let mut devs: [Recorder; 7] = Default::default();
        let [
            empty,
            overflow,
            top,
            serial,
            overlap_in,
            overlap_span,
            adjacent,
        ] = &mut devs;
        let overlap = Err(BusError::Overlap {
            existing_base: 0x3F8,
        });
        let mut bus = PortBus::new();
        assert_eq!(bus.attach(0x10, 0, empty), Err(BusError::EmptyRange));
        assert_eq!(
            bus.attach(0xFFFF, 2, overflow),
            Err(BusError::RangeOverflow)
        );
        bus.attach(0xFFF8, 8, top).unwrap(); // may own 0xFFFF
        bus.attach(0x3F8, 8, serial).unwrap();
        assert_eq!(bus.attach(0x3FF, 1, overlap_in), overlap);
        assert_eq!(bus.attach(0x3F0, 9, overlap_span), overlap);
        bus.attach(0x3F0, 8, adjacent).unwrap(); // adjacent is fine
        assert_eq!(bus.read(0xFFFF, AccessSize::Byte), 0x47);
    }
}
