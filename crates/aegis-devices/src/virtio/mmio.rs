//! virtio-mmio transport, version 2 (virtio 1.2, §4.2.2).
//!
//! [`VirtioMmio`] exposes a [`VirtioDevice`] as an [`MmioDevice`]. The
//! driver-initialization sequence (§3.1.1) is enforced:
//! - the status byte only gains bits, except through reset (writing 0);
//! - `FEATURES_OK` sticks only if the driver accepted `VIRTIO_F_VERSION_1`
//!   and nothing that was not offered; otherwise it reads back clear, so the
//!   driver sees the failure;
//! - queue configuration changes only while the queue is disabled;
//! - notifications count only after `DRIVER_OK`.
//!
//! A malformed queue sets `DEVICE_NEEDS_RESET` and raises a configuration
//! change interrupt (§2.1.2); the error is kept for diagnostics.
//!
//! Register accesses must be 32 bits wide (§4.2.2.2); other widths read as 0
//! and are ignored on write. The interrupt line stays asserted while any
//! `InterruptStatus` bit is set.

use alloc::vec::Vec;

use aegis_core::{AccessSize, GuestMemoryAccess, InterruptLine, MmioDevice};

use super::queue::{QueueError, SplitQueue};
use super::{VIRTIO_F_VERSION_1, VirtioDevice};

/// Size of the MMIO window the transport decodes.
pub const MMIO_SIZE: u64 = 0x1000;

const MAGIC_VALUE: u64 = 0x000;
const VERSION: u64 = 0x004;
const DEVICE_ID: u64 = 0x008;
const VENDOR_ID: u64 = 0x00C;
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_READY: u64 = 0x044;
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;
const QUEUE_DESC_LOW: u64 = 0x080;
const QUEUE_DESC_HIGH: u64 = 0x084;
const QUEUE_DRIVER_LOW: u64 = 0x090;
const QUEUE_DRIVER_HIGH: u64 = 0x094;
const QUEUE_DEVICE_LOW: u64 = 0x0A0;
const QUEUE_DEVICE_HIGH: u64 = 0x0A4;
const CONFIG: u64 = 0x100;

/// "virt", little-endian.
const MAGIC: u32 = 0x7472_6976;
/// Arbitrary vendor ID: "AEGS", little-endian.
const AEGIS_VENDOR_ID: u32 = u32::from_le_bytes(*b"AEGS");

/// Device status bits (§2.1).
pub mod status {
    /// The guest noticed the device.
    pub const ACKNOWLEDGE: u32 = 1;
    /// The guest has a driver for it.
    pub const DRIVER: u32 = 2;
    /// The driver is ready.
    pub const DRIVER_OK: u32 = 4;
    /// Feature negotiation is complete.
    pub const FEATURES_OK: u32 = 8;
    /// The device hit an error and needs a reset.
    pub const DEVICE_NEEDS_RESET: u32 = 64;
    /// The guest gave up on the device.
    pub const FAILED: u32 = 128;
}

/// `InterruptStatus` bit: a used buffer was returned.
pub const INT_VRING: u32 = 1;
/// `InterruptStatus` bit: the configuration changed (here: needs reset).
pub const INT_CONFIG: u32 = 2;

fn set_half(word: &mut u64, high: bool, value: u32) {
    if high {
        *word = (*word & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
    } else {
        *word = (*word & 0xFFFF_FFFF_0000_0000) | u64::from(value);
    }
}

/// A [`VirtioDevice`] behind the virtio-mmio transport.
#[derive(Debug)]
pub struct VirtioMmio<D, M, I> {
    device: D,
    mem: M,
    irq: I,
    status: u32,
    device_features_sel: u32,
    driver_features_sel: u32,
    driver_features: u64,
    queue_sel: u32,
    queues: Vec<SplitQueue>,
    interrupt_status: u32,
    last_error: Option<QueueError>,
}

impl<D: VirtioDevice, M: GuestMemoryAccess, I: InterruptLine> VirtioMmio<D, M, I> {
    /// Wrap `device`, giving it access to guest memory `mem` and the
    /// interrupt line `irq`.
    pub fn new(device: D, mem: M, irq: I) -> Self {
        let queues = alloc::vec![SplitQueue::default(); device.queue_max_sizes().len()];
        Self {
            device,
            mem,
            irq,
            status: 0,
            device_features_sel: 0,
            driver_features_sel: 0,
            driver_features: 0,
            queue_sel: 0,
            queues,
            interrupt_status: 0,
            last_error: None,
        }
    }

    /// The wrapped device.
    pub const fn device(&self) -> &D {
        &self.device
    }

    /// The device status byte.
    pub const fn status(&self) -> u32 {
        self.status
    }

    /// The queue error that set `DEVICE_NEEDS_RESET`, if any.
    pub const fn last_error(&self) -> Option<QueueError> {
        self.last_error
    }

    fn offered_features(&self) -> u64 {
        self.device.device_features() | VIRTIO_F_VERSION_1
    }

    fn selected_queue(&mut self) -> Option<(&mut SplitQueue, u16)> {
        let index = usize::try_from(self.queue_sel).ok()?;
        let max = *self.device.queue_max_sizes().get(index)?;
        Some((self.queues.get_mut(index)?, max))
    }

    fn reset(&mut self) {
        self.device.reset();
        for q in &mut self.queues {
            q.reset();
        }
        self.status = 0;
        self.device_features_sel = 0;
        self.driver_features_sel = 0;
        self.driver_features = 0;
        self.queue_sel = 0;
        self.interrupt_status = 0;
        self.last_error = None;
        self.irq.set_level(false);
    }

    fn raise(&mut self, bit: u32) {
        self.interrupt_status |= bit;
        self.irq.set_level(true);
    }

    fn fail(&mut self, error: QueueError) {
        self.status |= status::DEVICE_NEEDS_RESET;
        self.last_error = Some(error);
        self.raise(INT_CONFIG);
    }

    fn write_status(&mut self, value: u32) {
        if value == 0 {
            self.reset();
            return;
        }
        // Bits can only be added; NEEDS_RESET is the device's to set.
        let mut new = (self.status | value) & !status::DEVICE_NEEDS_RESET
            | (self.status & status::DEVICE_NEEDS_RESET);
        if new & status::FEATURES_OK != 0 && self.status & status::FEATURES_OK == 0 {
            let acceptable = self.driver_features & VIRTIO_F_VERSION_1 != 0
                && self.driver_features & !self.offered_features() == 0;
            if !acceptable {
                new &= !status::FEATURES_OK;
            }
        }
        if new & status::DRIVER_OK != 0 && new & status::FEATURES_OK == 0 {
            new &= !status::DRIVER_OK;
        }
        self.status = new;
    }

    fn notify(&mut self, index: u32) {
        if self.status & status::DRIVER_OK == 0 || self.status & status::DEVICE_NEEDS_RESET != 0 {
            return;
        }
        let Some(i) = usize::try_from(index)
            .ok()
            .filter(|i| *i < self.queues.len())
        else {
            return;
        };
        if !self.queues[i].is_ready() {
            return;
        }
        let result = self
            .device
            .notify(i, &mut self.queues[i], &self.mem)
            .and_then(|returned| Ok(returned && self.queues[i].wants_interrupt(&self.mem)?));
        match result {
            Ok(true) => self.raise(INT_VRING),
            Ok(false) => {}
            Err(e) => self.fail(e),
        }
    }

    fn write_queue_ready(&mut self, value: u32) {
        let Some((queue, max)) = self.selected_queue() else {
            return;
        };
        if value == 0 {
            queue.disable();
            return;
        }
        if queue.is_ready() {
            // Already enabled: enabling again would rewind the ring indices
            // and replay buffers that were already returned.
            return;
        }
        if let Err(e) = queue.enable(max) {
            self.fail(e);
        }
    }

    fn write_queue_field(&mut self, offset: u64, value: u32) {
        let Some((queue, _)) = self.selected_queue() else {
            return;
        };
        if queue.is_ready() {
            return;
        }
        match offset {
            QUEUE_NUM => queue.size = u16::try_from(value).unwrap_or(0),
            QUEUE_DESC_LOW | QUEUE_DESC_HIGH => {
                set_half(&mut queue.desc, offset == QUEUE_DESC_HIGH, value);
            }
            QUEUE_DRIVER_LOW | QUEUE_DRIVER_HIGH => {
                set_half(&mut queue.avail, offset == QUEUE_DRIVER_HIGH, value);
            }
            QUEUE_DEVICE_LOW | QUEUE_DEVICE_HIGH => {
                set_half(&mut queue.used, offset == QUEUE_DEVICE_HIGH, value);
            }
            _ => {}
        }
    }
}

impl<D: VirtioDevice, M: GuestMemoryAccess, I: InterruptLine> MmioDevice for VirtioMmio<D, M, I> {
    fn read(&mut self, offset: u64, size: AccessSize) -> u64 {
        if offset >= CONFIG {
            return self.device.read_config(offset.saturating_sub(CONFIG), size);
        }
        if size != AccessSize::Dword {
            return 0;
        }
        let value: u32 = match offset {
            MAGIC_VALUE => MAGIC,
            VERSION => 2,
            DEVICE_ID => self.device.device_id(),
            VENDOR_ID => AEGIS_VENDOR_ID,
            DEVICE_FEATURES => match self.device_features_sel {
                0 => u32::try_from(self.offered_features() & 0xFFFF_FFFF).unwrap_or(0),
                1 => u32::try_from(self.offered_features() >> 32).unwrap_or(0),
                _ => 0,
            },
            QUEUE_NUM_MAX => self.selected_queue().map_or(0, |(_, max)| u32::from(max)),
            QUEUE_READY => self
                .selected_queue()
                .map_or(0, |(q, _)| u32::from(q.is_ready())),
            INTERRUPT_STATUS => self.interrupt_status,
            STATUS => self.status,
            // ConfigGeneration (0x0FC) stays 0: the configuration space never
            // changes. Write-only and unknown registers also read as 0.
            _ => 0,
        };
        u64::from(value)
    }

    fn write(&mut self, offset: u64, size: AccessSize, value: u64) {
        if offset >= CONFIG || size != AccessSize::Dword {
            // The console's configuration space is read-only.
            return;
        }
        let value = u32::try_from(value & 0xFFFF_FFFF).unwrap_or(0);
        match offset {
            DEVICE_FEATURES_SEL => self.device_features_sel = value,
            DRIVER_FEATURES_SEL => self.driver_features_sel = value,
            DRIVER_FEATURES if self.status & status::FEATURES_OK == 0 => {
                match self.driver_features_sel {
                    0 => set_half(&mut self.driver_features, false, value),
                    1 => set_half(&mut self.driver_features, true, value),
                    _ => {}
                }
            }
            QUEUE_SEL => self.queue_sel = value,
            QUEUE_READY => self.write_queue_ready(value),
            QUEUE_NOTIFY => self.notify(value),
            INTERRUPT_ACK => {
                self.interrupt_status &= !value;
                self.irq.set_level(self.interrupt_status != 0);
            }
            STATUS => self.write_status(value),
            QUEUE_NUM | QUEUE_DESC_LOW | QUEUE_DESC_HIGH | QUEUE_DRIVER_LOW | QUEUE_DRIVER_HIGH
            | QUEUE_DEVICE_LOW | QUEUE_DEVICE_HIGH => self.write_queue_field(offset, value),
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // test layouts use small fixed offsets
mod tests {
    use super::*;
    use crate::virtio::VirtioConsole;
    use crate::virtio::queue::VIRTQ_DESC_F_NEXT;
    use aegis_core::mock::{RecordingLine, VecMemory};

    const DW: AccessSize = AccessSize::Dword;
    // A guest-side layout for the transmit queue in mock RAM.
    const TX_DESC: u64 = 0x1000;
    const TX_AVAIL: u64 = 0x2000;
    const TX_USED: u64 = 0x3000;
    const DATA: u64 = 0x8000;

    type Dev<'a> = VirtioMmio<VirtioConsole, &'a VecMemory, &'a RecordingLine>;

    fn w(dev: &mut Dev<'_>, offset: u64, value: u64) {
        dev.write(offset, DW, value);
    }

    fn r(dev: &mut Dev<'_>, offset: u64) -> u64 {
        dev.read(offset, DW)
    }

    /// The Linux virtio-mmio driver's init sequence, with the transmit queue
    /// (index 1) configured at the given addresses.
    fn init(dev: &mut Dev<'_>, size: u32, desc: u64, avail: u64, used: u64) {
        w(dev, STATUS, 0);
        w(dev, STATUS, u64::from(status::ACKNOWLEDGE));
        w(dev, STATUS, u64::from(status::ACKNOWLEDGE | status::DRIVER));
        w(dev, DRIVER_FEATURES_SEL, 1);
        w(dev, DRIVER_FEATURES, 1); // VIRTIO_F_VERSION_1 (bit 32)
        w(dev, DRIVER_FEATURES_SEL, 0);
        w(dev, DRIVER_FEATURES, 0);
        w(
            dev,
            STATUS,
            u64::from(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK),
        );
        w(dev, QUEUE_SEL, 1);
        w(dev, QUEUE_NUM, u64::from(size));
        w(dev, QUEUE_DESC_LOW, desc & 0xFFFF_FFFF);
        w(dev, QUEUE_DESC_HIGH, desc >> 32);
        w(dev, QUEUE_DRIVER_LOW, avail & 0xFFFF_FFFF);
        w(dev, QUEUE_DRIVER_HIGH, avail >> 32);
        w(dev, QUEUE_DEVICE_LOW, used & 0xFFFF_FFFF);
        w(dev, QUEUE_DEVICE_HIGH, used >> 32);
        w(dev, QUEUE_READY, 1);
        w(
            dev,
            STATUS,
            u64::from(
                status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
            ),
        );
    }

    fn desc(mem: &VecMemory, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let mut raw = [0u8; 16];
        raw[..8].copy_from_slice(&addr.to_le_bytes());
        raw[8..12].copy_from_slice(&len.to_le_bytes());
        raw[12..14].copy_from_slice(&flags.to_le_bytes());
        raw[14..].copy_from_slice(&next.to_le_bytes());
        mem.write(TX_DESC + u64::from(i) * 16, &raw).unwrap();
    }

    fn publish(mem: &VecMemory, slot: u16, head: u16, idx: u16) {
        mem.write(TX_AVAIL + 4 + u64::from(slot) * 2, &head.to_le_bytes())
            .unwrap();
        mem.write(TX_AVAIL + 2, &idx.to_le_bytes()).unwrap();
    }

    fn used_idx(mem: &VecMemory) -> u16 {
        let mut b = [0u8; 2];
        mem.read(TX_USED + 2, &mut b).unwrap();
        u16::from_le_bytes(b)
    }

    #[test]
    fn identification_registers() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        assert_eq!(r(&mut dev, MAGIC_VALUE), 0x7472_6976);
        assert_eq!(r(&mut dev, VERSION), 2);
        assert_eq!(r(&mut dev, DEVICE_ID), 3);
        assert_eq!(
            r(&mut dev, VENDOR_ID),
            u64::from(u32::from_le_bytes(*b"AEGS"))
        );
        w(&mut dev, DEVICE_FEATURES_SEL, 1);
        assert_eq!(
            r(&mut dev, DEVICE_FEATURES),
            1,
            "VIRTIO_F_VERSION_1 offered"
        );
        w(&mut dev, DEVICE_FEATURES_SEL, 0);
        assert_eq!(r(&mut dev, DEVICE_FEATURES), 0, "no device features");
        w(&mut dev, QUEUE_SEL, 1);
        assert_eq!(r(&mut dev, QUEUE_NUM_MAX), 256);
        w(&mut dev, QUEUE_SEL, 2);
        assert_eq!(r(&mut dev, QUEUE_NUM_MAX), 0, "only two queues");
        assert_eq!(
            dev.read(MAGIC_VALUE, AccessSize::Byte),
            0,
            "registers need 32-bit access"
        );
        assert_eq!(
            dev.read(CONFIG, AccessSize::Word),
            0,
            "config space reads as zero"
        );
    }

    #[test]
    fn transmitted_bytes_reach_the_output_and_raise_the_interrupt() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        assert_eq!(dev.status() & status::DRIVER_OK, status::DRIVER_OK);

        mem.write(DATA, b"hello, ").unwrap();
        mem.write(DATA + 0x100, b"world\n").unwrap();
        desc(&mem, 0, DATA, 7, VIRTQ_DESC_F_NEXT, 1);
        desc(&mem, 1, DATA + 0x100, 6, 0, 0);
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);

        assert_eq!(dev.device().output(), b"hello, world\n");
        assert_eq!(used_idx(&mem), 1);
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_VRING));
        assert!(line.asserted());
        w(&mut dev, INTERRUPT_ACK, u64::from(INT_VRING));
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), 0);
        assert!(
            !line.asserted(),
            "acknowledged interrupts deassert the line"
        );
    }

    #[test]
    fn features_ok_requires_version_1_and_only_offered_bits() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        let handshake = u64::from(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);

        w(
            &mut dev,
            STATUS,
            u64::from(status::ACKNOWLEDGE | status::DRIVER),
        );
        w(&mut dev, STATUS, handshake); // no VERSION_1 accepted
        assert_eq!(
            dev.status() & status::FEATURES_OK,
            0,
            "legacy drivers are refused"
        );

        w(&mut dev, STATUS, 0);
        w(&mut dev, DRIVER_FEATURES_SEL, 1);
        w(&mut dev, DRIVER_FEATURES, 1 | 2); // VERSION_1 plus bit 33, never offered
        w(&mut dev, STATUS, handshake);
        assert_eq!(
            dev.status() & status::FEATURES_OK,
            0,
            "unoffered feature refused"
        );

        w(&mut dev, STATUS, 0);
        w(&mut dev, DRIVER_FEATURES_SEL, 1);
        w(&mut dev, DRIVER_FEATURES, 1);
        w(&mut dev, STATUS, handshake);
        assert_eq!(dev.status() & status::FEATURES_OK, status::FEATURES_OK);
        w(&mut dev, DRIVER_FEATURES, 3);
        w(&mut dev, STATUS, handshake | u64::from(status::DRIVER_OK));
        assert_eq!(
            dev.status() & status::DRIVER_OK,
            status::DRIVER_OK,
            "features frozen after FEATURES_OK"
        );
    }

    #[test]
    fn driver_ok_without_features_ok_and_early_notify_are_ignored() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        w(
            &mut dev,
            STATUS,
            u64::from(status::ACKNOWLEDGE | status::DRIVER | status::DRIVER_OK),
        );
        assert_eq!(dev.status() & status::DRIVER_OK, 0);
        mem.write(DATA, b"x").unwrap();
        desc(&mem, 0, DATA, 1, 0, 0);
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert!(dev.device().output().is_empty());
        assert!(line.levels().is_empty(), "no interrupt before DRIVER_OK");
    }

    #[test]
    fn invalid_queue_configuration_needs_reset() {
        for (size, desc_addr) in [
            (6u32, TX_DESC),
            (8, TX_DESC + 4),
            (512, TX_DESC),
            (0x1_0000, TX_DESC),
        ] {
            let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
            let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
            init(&mut dev, size, desc_addr, TX_AVAIL, TX_USED);
            w(&mut dev, QUEUE_SEL, 1);
            assert_eq!(
                r(&mut dev, QUEUE_READY),
                0,
                "size {size}, desc {desc_addr:#x}"
            );
            assert_ne!(dev.status() & status::DEVICE_NEEDS_RESET, 0);
            assert!(dev.last_error().is_some());
            assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_CONFIG));
            assert!(line.asserted());
        }
    }

    #[test]
    fn malformed_ring_needs_reset_and_stops_processing() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        desc(&mem, 0, 0xFFFF_0000, 16, 0, 0); // buffer outside guest RAM
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert!(matches!(dev.last_error(), Some(QueueError::Memory(_))));
        assert_ne!(dev.status() & status::DEVICE_NEEDS_RESET, 0);
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_CONFIG));

        // Further notifications are ignored until the driver resets the device.
        mem.write(DATA, b"late").unwrap();
        desc(&mem, 1, DATA, 4, 0, 0);
        publish(&mem, 1, 1, 2);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert!(dev.device().output().is_empty());

        w(&mut dev, STATUS, 0);
        assert_eq!(dev.status(), 0);
        assert_eq!(dev.last_error(), None);
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), 0);
        assert!(!line.asserted(), "reset deasserts the line");
        w(&mut dev, QUEUE_SEL, 1);
        assert_eq!(r(&mut dev, QUEUE_READY), 0, "reset disables queues");
    }

    #[test]
    fn queue_config_is_frozen_while_ready_and_kept_on_disable() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        w(&mut dev, QUEUE_SEL, 1);
        w(&mut dev, QUEUE_DESC_LOW, 0x9000); // ignored: queue is ready
        mem.write(DATA, b"ok").unwrap();
        desc(&mem, 0, DATA, 2, 0, 0);
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert_eq!(
            dev.device().output(),
            b"ok",
            "still using the original descriptor table"
        );
        w(&mut dev, QUEUE_READY, 1); // already ready: must not rewind
        mem.write(DATA + 0x10, b"!").unwrap();
        desc(&mem, 1, DATA + 0x10, 1, 0, 0);
        publish(&mem, 1, 1, 2);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert_eq!(dev.device().output(), b"ok!", "nothing replayed");
        assert_eq!(used_idx(&mem), 2);
        w(&mut dev, QUEUE_READY, 0);
        assert_eq!(r(&mut dev, QUEUE_READY), 0);
        w(&mut dev, QUEUE_READY, 1);
        assert_eq!(
            r(&mut dev, QUEUE_READY),
            1,
            "configuration survived the disable"
        );
    }

    #[test]
    fn no_interrupt_flag_suppresses_the_interrupt() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        mem.write(TX_AVAIL, &1u16.to_le_bytes()).unwrap(); // VIRTQ_AVAIL_F_NO_INTERRUPT
        mem.write(DATA, b"q").unwrap();
        desc(&mem, 0, DATA, 1, 0, 0);
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert_eq!(dev.device().output(), b"q");
        assert_eq!(used_idx(&mem), 1, "buffer still returned");
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), 0);
        assert!(!line.asserted());
    }

    #[test]
    fn receive_queue_and_bad_indices_do_nothing() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        for queue in [0, 2, u64::from(u32::MAX)] {
            w(&mut dev, QUEUE_NOTIFY, queue);
        }
        assert_eq!(dev.status() & status::DEVICE_NEEDS_RESET, 0);
        assert!(!line.levels().contains(&true), "never asserted");
        dev.write(STATUS, AccessSize::Byte, 0); // wrong width: ignored
        assert_ne!(dev.status(), 0);
    }

    #[test]
    fn console_output_is_capped() {
        let (mem, line) = (VecMemory::new(0x10000), RecordingLine::default());
        let mut dev = VirtioMmio::new(VirtioConsole::with_capacity(4), &mem, &line);
        init(&mut dev, 8, TX_DESC, TX_AVAIL, TX_USED);
        mem.write(DATA, b"0123456789").unwrap();
        desc(&mem, 0, DATA, 10, 0, 0);
        publish(&mem, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert_eq!(dev.device().output(), b"0123");
        assert_eq!(dev.device().dropped(), 6);
    }
}
