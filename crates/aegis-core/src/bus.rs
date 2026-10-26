//! Device bus: routes guest port I/O and MMIO to devices.
//!
//! Devices are borrowed, not owned, so the caller can inspect them (for
//! example, read a serial port's output) after the run. They must be `Send`,
//! so an SMP runner can share the bus between vCPU threads behind a lock.
//!
//! - Unassigned ports behave like an empty ISA bus: reads return all ones and
//!   writes are dropped. Both are recorded in [`UnassignedLog`] and never stop
//!   the guest (roadmap M2 policy).
//! - Unclaimed MMIO is reported back to the vCPU, which ends the run with
//!   [`crate::StopReason::UnhandledMmio`]. A guest touching non-existent
//!   memory-mapped hardware is a configuration error, not something to paper
//!   over.
//!
//! Guest memory and interrupt lines are reached through [`GuestMemoryAccess`]
//! and [`InterruptLine`], which backends implement.

use alloc::vec::Vec;
use core::fmt;

use crate::backend::{MmioIo, PortIo};
use crate::exit::{AccessSize, IoAccess};

/// A device on the port bus. `offset` is relative to the device's base port.
pub trait PortDevice {
    /// A guest read of `size` bytes at `offset`.
    fn read(&mut self, offset: u16, size: AccessSize) -> u32;
    /// A guest write of `value` (masked to `size`) at `offset`.
    fn write(&mut self, offset: u16, size: AccessSize, value: u32);
}

/// A memory-mapped device. `offset` is relative to the device's base address.
pub trait MmioDevice {
    /// A guest read of `size` bytes at `offset`.
    fn read(&mut self, offset: u64, size: AccessSize) -> u64;
    /// A guest write of `value` (masked to `size`) at `offset`.
    fn write(&mut self, offset: u64, size: AccessSize, value: u64);
}

/// A failed guest-memory access by a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestAccessError {
    /// Start of the rejected range.
    pub gpa: u64,
    /// Its length.
    pub len: u64,
}

impl fmt::Display for GuestAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:#x} bytes at {:#x} are outside guest memory",
            self.len, self.gpa
        )
    }
}

impl core::error::Error for GuestAccessError {}

/// Device access to guest RAM (DMA). Implementations must reject any range
/// not wholly inside guest memory, never truncate it.
pub trait GuestMemoryAccess {
    /// Copy guest memory at `gpa` into `buf`.
    ///
    /// # Errors
    ///
    /// [`GuestAccessError`] if `[gpa, gpa + buf.len())` is not guest RAM.
    fn read(&self, gpa: u64, buf: &mut [u8]) -> Result<(), GuestAccessError>;
    /// Copy `data` into guest memory at `gpa`.
    ///
    /// # Errors
    ///
    /// [`GuestAccessError`] if `[gpa, gpa + data.len())` is not guest RAM.
    fn write(&self, gpa: u64, data: &[u8]) -> Result<(), GuestAccessError>;
}

impl<T: GuestMemoryAccess + ?Sized> GuestMemoryAccess for &T {
    fn read(&self, gpa: u64, buf: &mut [u8]) -> Result<(), GuestAccessError> {
        (**self).read(gpa, buf)
    }
    fn write(&self, gpa: u64, data: &[u8]) -> Result<(), GuestAccessError> {
        (**self).write(gpa, data)
    }
}

/// A level-triggered interrupt line into the guest.
pub trait InterruptLine {
    /// Assert (`true`) or deassert (`false`) the line.
    fn set_level(&self, asserted: bool);
}

impl<T: InterruptLine + ?Sized> InterruptLine for &T {
    fn set_level(&self, asserted: bool) {
        (**self).set_level(asserted);
    }
}

/// Where a device's output goes as the guest produces it: a capped
/// in-memory buffer by default, or a host stream to a terminal.
///
/// Devices call this on a vCPU thread with the device bus locked, so an
/// implementation must not block for long. It should queue what it can and
/// drop and count the rest.
pub trait OutputSink {
    /// The guest produced `bytes`, in order.
    fn output(&mut self, bytes: &[u8]);
}

/// Why a device could not be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusError {
    /// `len` was zero.
    EmptyRange,
    /// `base + len` would pass the end of the address space.
    RangeOverflow,
    /// The range overlaps an attached device.
    Overlap {
        /// Base address of the device already attached there.
        existing_base: u64,
    },
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => write!(f, "device range is empty"),
            Self::RangeOverflow => {
                write!(f, "device range extends past the end of the address space")
            }
            Self::Overlap { existing_base } => {
                write!(f, "device range overlaps the device at {existing_base:#x}")
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

struct PortSlot<'d> {
    base: u16,
    /// Inclusive, so a device can own port 0xFFFF.
    last: u16,
    device: &'d mut (dyn PortDevice + Send),
}

struct MmioSlot<'d> {
    base: u64,
    /// Inclusive, so a device can own the top of the address space.
    last: u64,
    device: &'d mut (dyn MmioDevice + Send),
}

/// Routes port I/O and MMIO to borrowed devices.
#[derive(Default)]
pub struct DeviceBus<'d> {
    ports: Vec<PortSlot<'d>>,
    mmio: Vec<MmioSlot<'d>>,
    unassigned: UnassignedLog,
}

impl fmt::Debug for DeviceBus<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceBus")
            .field(
                "ports",
                &self
                    .ports
                    .iter()
                    .map(|d| (d.base, d.last))
                    .collect::<Vec<_>>(),
            )
            .field(
                "mmio",
                &self
                    .mmio
                    .iter()
                    .map(|d| (d.base, d.last))
                    .collect::<Vec<_>>(),
            )
            .field("unassigned", &self.unassigned)
            .finish()
    }
}

impl<'d> DeviceBus<'d> {
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
    /// attached port device.
    pub fn attach_port(
        &mut self,
        base: u16,
        len: u16,
        device: &'d mut (dyn PortDevice + Send),
    ) -> Result<(), BusError> {
        let span = len.checked_sub(1).ok_or(BusError::EmptyRange)?;
        let last = base.checked_add(span).ok_or(BusError::RangeOverflow)?;
        if let Some(existing) = self.ports.iter().find(|d| base <= d.last && d.base <= last) {
            return Err(BusError::Overlap {
                existing_base: u64::from(existing.base),
            });
        }
        self.ports.push(PortSlot { base, last, device });
        Ok(())
    }

    /// Attach `device` at guest-physical `base..base + len`.
    ///
    /// # Errors
    ///
    /// [`BusError`] if the range is empty, wraps the address space, or
    /// overlaps an attached MMIO device.
    pub fn attach_mmio(
        &mut self,
        base: u64,
        len: u64,
        device: &'d mut (dyn MmioDevice + Send),
    ) -> Result<(), BusError> {
        let span = len.checked_sub(1).ok_or(BusError::EmptyRange)?;
        let last = base.checked_add(span).ok_or(BusError::RangeOverflow)?;
        if let Some(existing) = self.mmio.iter().find(|d| base <= d.last && d.base <= last) {
            return Err(BusError::Overlap {
                existing_base: existing.base,
            });
        }
        self.mmio.push(MmioSlot { base, last, device });
        Ok(())
    }

    /// Accesses that hit no port device.
    #[must_use]
    pub fn unassigned(&self) -> &UnassignedLog {
        &self.unassigned
    }

    /// The port device covering `port`, and `port`'s offset within it.
    fn route_port(&mut self, port: u16) -> Option<(&mut (dyn PortDevice + Send + 'd), u16)> {
        self.ports
            .iter_mut()
            .find(|d| d.base <= port && port <= d.last)
            .and_then(|d| Some((&mut *d.device, port.checked_sub(d.base)?)))
    }

    /// The MMIO device wholly containing `[gpa, gpa + size)`, and the offset.
    /// An access straddling a device's edge is not claimed by it.
    fn route_mmio(
        &mut self,
        gpa: u64,
        size: AccessSize,
    ) -> Option<(&mut (dyn MmioDevice + Send + 'd), u64)> {
        let last_byte = gpa.checked_add(u64::from(size.bytes()).saturating_sub(1))?;
        self.mmio
            .iter_mut()
            .find(|d| d.base <= gpa && last_byte <= d.last)
            .and_then(|d| Some((&mut *d.device, gpa.checked_sub(d.base)?)))
    }
}

impl PortIo for DeviceBus<'_> {
    fn read(&mut self, port: u16, size: AccessSize) -> u32 {
        if let Some((device, offset)) = self.route_port(port) {
            return device.read(offset, size);
        }
        self.unassigned.record(port, size, None);
        // All ones for the access width; a Dword mask fits in u32.
        u32::try_from(size.mask()).unwrap_or(u32::MAX)
    }

    fn write(&mut self, port: u16, size: AccessSize, value: u32) {
        if let Some((device, offset)) = self.route_port(port) {
            device.write(offset, size, value);
            return;
        }
        self.unassigned.record(port, size, Some(value));
    }
}

impl MmioIo for DeviceBus<'_> {
    fn mmio_read(&mut self, gpa: u64, size: AccessSize) -> Option<u64> {
        let (device, offset) = self.route_mmio(gpa, size)?;
        Some(device.read(offset, size) & size.mask())
    }

    fn mmio_write(&mut self, gpa: u64, size: AccessSize, value: u64) -> bool {
        match self.route_mmio(gpa, size) {
            Some((device, offset)) => {
                device.write(offset, size, value & size.mask());
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Remembers every access; reads return `0x40 + offset`.
    #[derive(Default)]
    struct Recorder {
        writes: Vec<(u64, u64)>,
        reads: Vec<u64>,
    }

    impl PortDevice for Recorder {
        fn read(&mut self, offset: u16, _size: AccessSize) -> u32 {
            self.reads.push(u64::from(offset));
            0x40 + u32::from(offset)
        }
        fn write(&mut self, offset: u16, _size: AccessSize, value: u32) {
            self.writes.push((u64::from(offset), u64::from(value)));
        }
    }

    impl MmioDevice for Recorder {
        fn read(&mut self, offset: u64, _size: AccessSize) -> u64 {
            self.reads.push(offset);
            0x1122_3344_5566_7788
        }
        fn write(&mut self, offset: u64, _size: AccessSize, value: u64) {
            self.writes.push((offset, value));
        }
    }

    #[test]
    fn routes_by_offset_within_range() {
        let mut dev = Recorder::default();
        {
            let mut bus = DeviceBus::new();
            bus.attach_port(0x3F8, 8, &mut dev).unwrap();
            bus.write(0x3F8, AccessSize::Byte, 0x41);
            bus.write(0x3FF, AccessSize::Byte, 0x7);
            assert_eq!(PortIo::read(&mut bus, 0x3FD, AccessSize::Byte), 0x45);
            assert_eq!(bus.unassigned().count, 0);
        }
        assert_eq!(dev.writes, [(0, 0x41), (7, 0x7)]);
        assert_eq!(dev.reads, [5]);
    }

    #[test]
    fn unassigned_reads_are_all_ones_and_both_directions_are_logged() {
        let mut bus = DeviceBus::new();
        assert_eq!(PortIo::read(&mut bus, 0x80, AccessSize::Byte), 0xFF);
        assert_eq!(PortIo::read(&mut bus, 0x80, AccessSize::Word), 0xFFFF);
        assert_eq!(PortIo::read(&mut bus, 0x80, AccessSize::Dword), 0xFFFF_FFFF);
        bus.write(0x404, AccessSize::Byte, 0x12);
        let log = bus.unassigned();
        assert_eq!(log.count, 4);
        assert_eq!(log.first[0].write_value(), None);
        assert_eq!(log.first[3].port(), 0x404);
        assert_eq!(log.first[3].write_value(), Some(0x12));
    }

    #[test]
    fn unassigned_log_keeps_only_the_first_accesses() {
        let mut bus = DeviceBus::new();
        for port in 0..100u16 {
            bus.write(port, AccessSize::Byte, 0);
        }
        assert_eq!(bus.unassigned().count, 100);
        assert_eq!(bus.unassigned().first.len(), UnassignedLog::KEEP);
        assert_eq!(bus.unassigned().first[15].port(), 15);
    }

    #[test]
    fn attach_port_rejects_bad_ranges() {
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
        let mut bus = DeviceBus::new();
        assert_eq!(bus.attach_port(0x10, 0, empty), Err(BusError::EmptyRange));
        assert_eq!(
            bus.attach_port(0xFFFF, 2, overflow),
            Err(BusError::RangeOverflow)
        );
        bus.attach_port(0xFFF8, 8, top).unwrap(); // may own 0xFFFF
        bus.attach_port(0x3F8, 8, serial).unwrap();
        assert_eq!(bus.attach_port(0x3FF, 1, overlap_in), overlap);
        assert_eq!(bus.attach_port(0x3F0, 9, overlap_span), overlap);
        bus.attach_port(0x3F0, 8, adjacent).unwrap(); // adjacent is fine
        assert_eq!(PortIo::read(&mut bus, 0xFFFF, AccessSize::Byte), 0x47);
    }

    #[test]
    fn mmio_routes_whole_accesses_and_masks_values() {
        let mut dev = Recorder::default();
        {
            let mut bus = DeviceBus::new();
            bus.attach_mmio(0xD000_0000, 0x1000, &mut dev).unwrap();
            assert_eq!(
                bus.mmio_read(0xD000_0000, AccessSize::Dword),
                Some(0x5566_7788)
            );
            assert_eq!(
                bus.mmio_read(0xD000_0FF8, AccessSize::Qword),
                Some(0x1122_3344_5566_7788)
            );
            assert!(bus.mmio_write(0xD000_0070, AccessSize::Byte, 0x1FF));
            assert_eq!(
                bus.mmio_read(0xD000_0FFE, AccessSize::Dword),
                None,
                "straddles the end"
            );
            assert_eq!(
                bus.mmio_read(0xCFFF_FFFF, AccessSize::Word),
                None,
                "straddles the start"
            );
            assert_eq!(
                bus.mmio_read(0xE000_0000, AccessSize::Byte),
                None,
                "no device"
            );
            assert!(!bus.mmio_write(0xE000_0000, AccessSize::Byte, 1));
            assert_eq!(bus.unassigned().count, 0, "MMIO misses are not port misses");
        }
        assert_eq!(dev.reads, [0, 0xFF8]);
        assert_eq!(
            dev.writes,
            [(0x70, 0xFF)],
            "write value masked to the access size"
        );
    }

    #[test]
    fn attach_mmio_rejects_bad_ranges() {
        let mut devs: [Recorder; 5] = Default::default();
        let [empty, wrap, top, first, overlap] = &mut devs;
        let mut bus = DeviceBus::new();
        assert_eq!(bus.attach_mmio(0x1000, 0, empty), Err(BusError::EmptyRange));
        assert_eq!(
            bus.attach_mmio(u64::MAX, 2, wrap),
            Err(BusError::RangeOverflow)
        );
        bus.attach_mmio(u64::MAX - 0xFFF, 0x1000, top).unwrap(); // may own the top
        bus.attach_mmio(0xD000_0000, 0x1000, first).unwrap();
        assert_eq!(
            bus.attach_mmio(0xD000_0FFF, 0x10, overlap),
            Err(BusError::Overlap {
                existing_base: 0xD000_0000
            })
        );
        assert_eq!(
            bus.mmio_read(u64::MAX - 7, AccessSize::Qword),
            Some(0x1122_3344_5566_7788)
        );
    }
}
