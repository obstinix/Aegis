//! virtio-console (virtio 1.2, §5.3), device ID 3: one port, no multiport.
//!
//! - Queue 0, `receiveq`: host-to-guest. Host input is queued with
//!   [`VirtioConsole::queue_input`] in a FIFO of at most
//!   [`VirtioConsole::DEFAULT_INPUT_CAPACITY`] bytes; the caller keeps what
//!   does not fit (backpressure). Queued bytes are written, in order, into
//!   the driver's receive buffers, filling each one before the next. The last
//!   one may be partly filled, and its used length says how much. A buffer
//!   is taken from the ring only when there is input for it, so posted
//!   buffers wait for input instead of being returned empty.
//! - Queue 1, `transmitq`: guest-to-host. Device-readable buffers are
//!   appended to the output, which is capped at
//!   [`VirtioConsole::DEFAULT_CAPACITY`] bytes; excess is counted and
//!   dropped. Every chain is returned to the used ring with length 0.
//!
//! **Input waits for the guest's first write to the port.** Linux's
//! virtio-console driver without `VIRTIO_CONSOLE_F_MULTIPORT` posts its
//! receive buffers in `add_port()` before it registers the port, and
//! `in_intr()` frees, data and all, any buffer that comes back for a port it
//! cannot find yet. Input delivered as soon as buffers appear is therefore
//! lost (seen live: every byte written, none read). The device has no other
//! sign that the port is registered without the multiport control queue.
//! The first transmit is such a sign, since the guest can only write to a
//! registered port. So until the guest transmits, input stays queued.
//! Then the receive queue is processed in the same exit, and from then on
//! whenever the driver notifies it (new buffers) or the host calls
//! [`VirtioMmio::input`] (new input). A reset closes it again. Input
//! survives the reset, so a driver that reinitializes the device still
//! receives it.
//!
//! No feature bits are offered (no `VIRTIO_CONSOLE_F_SIZE`, `F_MULTIPORT` or
//! `F_EMERG_WRITE`), so the configuration space reads as zero.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use aegis_core::{AccessSize, GuestMemoryAccess, InterruptLine};

use super::queue::{QueueError, SplitQueue};
use super::{VirtioDevice, VirtioMmio};

/// Virtio device ID for console.
pub const DEVICE_ID: u32 = 3;
/// Index of the host-to-guest queue.
pub const RECEIVEQ: usize = 0;
/// Index of the guest-to-host queue.
pub const TRANSMITQ: usize = 1;
const QUEUE_SIZE: u16 = 256;

/// A virtio console whose guest output is collected in memory and whose
/// input comes from the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtioConsole {
    output: Vec<u8>,
    capacity: usize,
    dropped: u64,
    input: VecDeque<u8>,
    input_capacity: usize,
    delivered: u64,
    /// The guest has transmitted since the last reset, so its port is
    /// registered and receive buffers may be filled.
    port_open: bool,
    /// Process the receive queue next (set when the port opens).
    receive_requested: bool,
}

impl Default for VirtioConsole {
    fn default() -> Self {
        Self::with_limits(Self::DEFAULT_CAPACITY, Self::DEFAULT_INPUT_CAPACITY)
    }
}

impl VirtioConsole {
    /// Default output cap in bytes.
    pub const DEFAULT_CAPACITY: usize = 64 * 1024;
    /// Default bound on input waiting for receive buffers, in bytes.
    pub const DEFAULT_INPUT_CAPACITY: usize = 4096;

    /// A console keeping at most `capacity` output bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limits(capacity, Self::DEFAULT_INPUT_CAPACITY)
    }

    /// A console keeping at most `output` output bytes and holding at most
    /// `input` bytes of undelivered input.
    #[must_use]
    pub fn with_limits(output: usize, input: usize) -> Self {
        Self {
            output: Vec::new(),
            capacity: output,
            dropped: 0,
            input: VecDeque::new(),
            input_capacity: input,
            delivered: 0,
            port_open: false,
            receive_requested: false,
        }
    }

    /// Whether the guest has written to the port since the last reset,
    /// which opens the receive path (see the module docs).
    #[must_use]
    pub const fn port_open(&self) -> bool {
        self.port_open
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

    /// Queue host input for the guest. Takes as many leading bytes of `data`
    /// as fit and returns how many; the caller keeps the rest and offers it
    /// again later. Nothing is delivered until the receive queue is processed
    /// (see [`VirtioMmio::input`]).
    pub fn queue_input(&mut self, data: &[u8]) -> usize {
        let room = self.input_capacity.saturating_sub(self.input.len());
        let taken = data.len().min(room);
        self.input.extend(&data[..taken]);
        taken
    }

    /// Input bytes waiting for receive buffers.
    #[must_use]
    pub fn pending_input(&self) -> usize {
        self.input.len()
    }

    /// Input bytes written into guest receive buffers so far.
    #[must_use]
    pub const fn delivered_input(&self) -> u64 {
        self.delivered
    }

    fn append(&mut self, data: &[u8]) {
        let room = self.capacity.saturating_sub(self.output.len());
        let keep = data.len().min(room);
        self.output.extend_from_slice(&data[..keep]);
        let lost = u64::try_from(data.len().saturating_sub(keep)).unwrap_or(u64::MAX);
        self.dropped = self.dropped.saturating_add(lost);
    }

    fn transmit(
        &mut self,
        queue: &mut SplitQueue,
        mem: &dyn GuestMemoryAccess,
    ) -> Result<bool, QueueError> {
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
        if returned && !self.port_open {
            self.port_open = true;
            self.receive_requested = !self.input.is_empty();
        }
        Ok(returned)
    }

    /// Move queued input into receive buffers until either runs out. Nothing
    /// is taken from the ring until the port is open.
    fn receive(
        &mut self,
        queue: &mut SplitQueue,
        mem: &dyn GuestMemoryAccess,
    ) -> Result<bool, QueueError> {
        if !self.port_open {
            return Ok(false);
        }
        let mut returned = false;
        // At most one pass over the ring per call: a driver that keeps
        // posting buffers from another vCPU cannot hold the device here.
        for _ in 0..queue.size {
            if self.input.is_empty() {
                break;
            }
            let Some(chain) = queue.pop(mem)? else {
                break;
            };
            // Validate the whole chain before writing any of it.
            if chain.buffers.iter().any(|b| !b.writable) {
                return Err(QueueError::NotWritable);
            }
            // Write the whole chain first. The bytes leave the FIFO only once
            // all of them are in guest memory, so a failed write, even in a
            // later buffer of the chain, loses nothing.
            let queued = self.input.make_contiguous();
            let mut taken = 0usize;
            for buffer in &chain.buffers {
                let rest = &queued[taken..];
                let n = usize::try_from(buffer.len)
                    .unwrap_or(usize::MAX)
                    .min(rest.len());
                if n == 0 {
                    continue;
                }
                mem.write(buffer.addr, &rest[..n])?;
                taken = taken.saturating_add(n);
            }
            self.input.drain(..taken);
            // `taken` is at most the chain's total length, capped at 1 MiB.
            let written = u32::try_from(taken).map_err(|_| QueueError::ChainTooLarge)?;
            self.delivered = self.delivered.saturating_add(u64::from(written));
            queue.push_used(mem, chain.head, written)?;
            returned = true;
        }
        Ok(returned)
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
        match index {
            RECEIVEQ => self.receive(queue, mem),
            TRANSMITQ => self.transmit(queue, mem),
            _ => Ok(false),
        }
    }

    fn take_requested_queue(&mut self) -> Option<usize> {
        core::mem::take(&mut self.receive_requested).then_some(RECEIVEQ)
    }

    fn reset(&mut self) {
        // A reinitialized driver registers its port again, so the receive
        // path closes until its next write. Output already delivered to the
        // host stays, and so does input not yet delivered.
        self.port_open = false;
        self.receive_requested = false;
    }
}

impl<M: GuestMemoryAccess, I: InterruptLine> VirtioMmio<VirtioConsole, M, I> {
    /// Offer host input to the guest: queue what fits (see
    /// [`VirtioConsole::queue_input`]), then process the receive queue, which
    /// fills any posted buffers and raises the used-buffer interrupt. Returns
    /// how many bytes of `data` were taken.
    pub fn input(&mut self, data: &[u8]) -> usize {
        let taken = self.device_mut().queue_input(data);
        self.process(RECEIVEQ);
        taken
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // test layouts use small fixed offsets
mod tests {
    use super::*;
    use crate::virtio::mmio::{INT_CONFIG, INT_VRING, status};
    use crate::virtio::queue::{MAX_CHAIN_BYTES, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use aegis_core::MmioDevice;
    use aegis_core::mock::{RecordingLine, VecMemory};

    // virtio-mmio register offsets (virtio 1.2, §4.2.2), written out here as
    // a guest driver sees them rather than taken from the transport.
    const DRIVER_FEATURES: u64 = 0x020;
    const DRIVER_FEATURES_SEL: u64 = 0x024;
    const QUEUE_SEL: u64 = 0x030;
    const QUEUE_NUM: u64 = 0x038;
    const QUEUE_READY: u64 = 0x044;
    const QUEUE_NOTIFY: u64 = 0x050;
    const INTERRUPT_STATUS: u64 = 0x060;
    const INTERRUPT_ACK: u64 = 0x064;
    const STATUS: u64 = 0x070;
    const QUEUE_DESC_LOW: u64 = 0x080;
    const QUEUE_DRIVER_LOW: u64 = 0x090;
    const QUEUE_DEVICE_LOW: u64 = 0x0A0;

    /// Where the guest put one queue's rings in mock RAM.
    #[derive(Clone, Copy)]
    struct Ring {
        desc: u64,
        avail: u64,
        used: u64,
    }

    const RX: Ring = Ring {
        desc: 0x1000,
        avail: 0x2000,
        used: 0x3000,
    };
    const TX: Ring = Ring {
        desc: 0x4000,
        avail: 0x5000,
        used: 0x6000,
    };
    /// The receive queue's rings after the driver reinitializes the device.
    const RX2: Ring = Ring {
        desc: 0x7000,
        avail: 0x7800,
        used: 0x7C00,
    };
    const BUFS: u64 = 0x8000;
    const SIZE: u16 = 8;

    type Dev<'a> = VirtioMmio<VirtioConsole, &'a VecMemory, &'a RecordingLine>;

    fn w(dev: &mut Dev<'_>, offset: u64, value: u64) {
        dev.write(offset, AccessSize::Dword, value);
    }

    fn r(dev: &mut Dev<'_>, offset: u64) -> u64 {
        dev.read(offset, AccessSize::Dword)
    }

    /// Reset, then negotiate `VIRTIO_F_VERSION_1` up to `FEATURES_OK`.
    fn handshake(dev: &mut Dev<'_>) {
        let ack_driver = u64::from(status::ACKNOWLEDGE | status::DRIVER);
        w(dev, STATUS, 0);
        w(dev, STATUS, ack_driver);
        w(dev, DRIVER_FEATURES_SEL, 1);
        w(dev, DRIVER_FEATURES, 1); // bit 32
        w(dev, STATUS, ack_driver | u64::from(status::FEATURES_OK));
    }

    fn configure(dev: &mut Dev<'_>, index: u64, ring: Ring) {
        w(dev, QUEUE_SEL, index);
        w(dev, QUEUE_NUM, u64::from(SIZE));
        w(dev, QUEUE_DESC_LOW, ring.desc);
        w(dev, QUEUE_DRIVER_LOW, ring.avail);
        w(dev, QUEUE_DEVICE_LOW, ring.used);
        w(dev, QUEUE_READY, 1);
    }

    fn driver_ok(dev: &mut Dev<'_>) {
        let all = status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK;
        w(dev, STATUS, u64::from(all));
    }

    /// The Linux driver's order: features, both queues, then `DRIVER_OK`.
    fn init(dev: &mut Dev<'_>) {
        handshake(dev);
        configure(dev, 0, RX);
        configure(dev, 1, TX);
        driver_ok(dev);
    }

    fn desc(mem: &VecMemory, ring: Ring, i: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let mut raw = [0u8; 16];
        raw[..8].copy_from_slice(&addr.to_le_bytes());
        raw[8..12].copy_from_slice(&len.to_le_bytes());
        raw[12..14].copy_from_slice(&flags.to_le_bytes());
        raw[14..].copy_from_slice(&next.to_le_bytes());
        mem.write(ring.desc + u64::from(i) * 16, &raw).unwrap();
    }

    /// Publish chain `head` in avail slot `slot` and set the avail index.
    fn post(mem: &VecMemory, ring: Ring, slot: u16, head: u16, idx: u16) {
        mem.write(ring.avail + 4 + u64::from(slot) * 2, &head.to_le_bytes())
            .unwrap();
        mem.write(ring.avail + 2, &idx.to_le_bytes()).unwrap();
    }

    fn buf(n: u16) -> u64 {
        BUFS + u64::from(n) * 0x100
    }

    /// Post receive buffer `n`: one device-writable descriptor of `len`
    /// bytes, in descriptor `n` and avail slot `n` (avail index `n + 1`).
    fn post_rx(mem: &VecMemory, n: u16, len: u32) {
        desc(mem, RX, n, buf(n), len, VIRTQ_DESC_F_WRITE, 0);
        post(mem, RX, n, n, n + 1);
    }

    fn used_idx(mem: &VecMemory, ring: Ring) -> u16 {
        let mut b = [0u8; 2];
        mem.read(ring.used + 2, &mut b).unwrap();
        u16::from_le_bytes(b)
    }

    /// Used-ring entry `slot`: (descriptor head, bytes written).
    fn used(mem: &VecMemory, ring: Ring, slot: u16) -> (u32, u32) {
        let mut b = [0u8; 8];
        mem.read(ring.used + 4 + u64::from(slot) * 8, &mut b)
            .unwrap();
        (
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        )
    }

    fn bytes(mem: &VecMemory, addr: u64, len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        mem.read(addr, &mut b).unwrap();
        b
    }

    /// The guest's first write to the port: one byte on the transmit queue
    /// (descriptor 7, avail slot 0), which opens the receive path. Its
    /// interrupt is acknowledged so that tests start from a clear line.
    fn open_port(dev: &mut Dev<'_>, mem: &VecMemory) {
        mem.write(buf(7), b"!").unwrap();
        desc(mem, TX, 7, buf(7), 1, 0, 0);
        post(mem, TX, 0, 7, 1);
        w(dev, QUEUE_NOTIFY, 1);
        w(dev, INTERRUPT_ACK, u64::from(INT_VRING));
        assert!(dev.device().port_open());
    }

    /// [`init`], then the guest's first write.
    fn init_open(dev: &mut Dev<'_>, mem: &VecMemory) {
        init(dev);
        open_port(dev, mem);
    }

    fn setup(console: VirtioConsole) -> (VecMemory, RecordingLine, VirtioConsole) {
        (VecMemory::new(0x10000), RecordingLine::default(), console)
    }

    #[test]
    fn input_fills_a_posted_receive_buffer_and_raises_the_interrupt() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        post_rx(&mem, 0, 64);
        w(&mut dev, QUEUE_NOTIFY, 0);
        assert_eq!(used_idx(&mem, RX), 0, "no input: the buffer stays posted");
        assert!(!line.asserted());

        assert_eq!(dev.input(b"hello"), 5);
        assert_eq!(used_idx(&mem, RX), 1);
        assert_eq!(used(&mem, RX, 0), (0, 5), "used length = bytes written");
        assert_eq!(bytes(&mem, buf(0), 5), b"hello");
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_VRING));
        assert!(line.asserted());
        assert_eq!(dev.device().pending_input(), 0);
        assert_eq!(dev.device().delivered_input(), 5);
    }

    #[test]
    fn input_without_receive_buffers_waits_for_the_driver() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        assert_eq!(dev.input(b"early"), 5, "queued, not dropped");
        assert_eq!(dev.device().pending_input(), 5);
        assert_eq!(used_idx(&mem, RX), 0);
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), 0, "nothing returned yet");
        assert!(!line.asserted());

        post_rx(&mem, 0, 64);
        w(&mut dev, QUEUE_NOTIFY, 0);
        assert_eq!(used(&mem, RX, 0), (0, 5));
        assert_eq!(bytes(&mem, buf(0), 5), b"early");
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_VRING));
    }

    /// Linux posts receive buffers (one kick each) before it registers the
    /// port, and frees any that come back before then. So input waits for
    /// the guest's first write, not just for buffers and `DRIVER_OK`.
    #[test]
    fn input_waits_for_the_guests_first_write_then_flows_in_the_same_exit() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        assert_eq!(dev.input(b"boot"), 4, "accepted before the driver exists");
        handshake(&mut dev);
        configure(&mut dev, 0, RX);
        configure(&mut dev, 1, TX);
        post_rx(&mem, 0, 64);
        w(&mut dev, QUEUE_NOTIFY, 0); // before DRIVER_OK: ignored
        assert_eq!(used_idx(&mem, RX), 0, "no buffer use before DRIVER_OK");
        driver_ok(&mut dev);
        post_rx(&mem, 1, 64);
        w(&mut dev, QUEUE_NOTIFY, 0); // as during the driver's add_port()
        assert_eq!(used_idx(&mem, RX), 0, "port not open: buffers wait");
        assert!(!dev.device().port_open());
        assert!(!line.levels().contains(&true), "no interrupt yet");

        // The guest's first write opens the port, and the receive queue is
        // processed right after the transmit queue, in the same exit.
        mem.write(buf(7), b"hi").unwrap();
        desc(&mem, TX, 0, buf(7), 2, 0, 0);
        post(&mem, TX, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert_eq!(dev.device().output(), b"hi");
        assert!(dev.device().port_open());
        assert_eq!(used(&mem, RX, 0), (0, 4));
        assert_eq!(bytes(&mem, buf(0), 4), b"boot");
        assert_eq!(used_idx(&mem, RX), 1, "the second buffer stays posted");
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), u64::from(INT_VRING));
        assert!(line.asserted());
    }

    #[test]
    fn long_input_spans_buffers_and_the_last_is_partly_filled() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        for n in 0..3 {
            post_rx(&mem, n, 4);
        }
        assert_eq!(dev.input(b"0123456789"), 10);
        assert_eq!(used_idx(&mem, RX), 3);
        assert_eq!(
            [used(&mem, RX, 0), used(&mem, RX, 1), used(&mem, RX, 2)],
            [(0, 4), (1, 4), (2, 2)]
        );
        assert_eq!(bytes(&mem, buf(0), 4), b"0123");
        assert_eq!(bytes(&mem, buf(1), 4), b"4567");
        assert_eq!(bytes(&mem, buf(2), 2), b"89");

        assert_eq!(dev.input(b"z"), 1);
        assert_eq!(used_idx(&mem, RX), 3, "a returned buffer is never reused");
        assert_eq!(dev.device().pending_input(), 1);
    }

    #[test]
    fn chained_receive_buffer_is_filled_in_order() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        desc(
            &mem,
            RX,
            0,
            buf(0),
            3,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            1,
        );
        desc(&mem, RX, 1, buf(1), 5, VIRTQ_DESC_F_WRITE, 0);
        post(&mem, RX, 0, 0, 1);
        assert_eq!(dev.input(b"abcdef"), 6);
        assert_eq!(used(&mem, RX, 0), (0, 6), "one used entry for the chain");
        assert_eq!(bytes(&mem, buf(0), 3), b"abc");
        assert_eq!(bytes(&mem, buf(1), 3), b"def");
    }

    #[test]
    fn input_is_bounded_and_the_rest_is_left_to_the_caller() {
        let (mem, line, console) = setup(VirtioConsole::with_limits(64, 8));
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        assert_eq!(dev.input(b"0123456789"), 8, "only what fits");
        assert_eq!(dev.input(b"x"), 0, "full");
        assert_eq!(dev.device().pending_input(), 8);

        post_rx(&mem, 0, 4);
        w(&mut dev, QUEUE_NOTIFY, 0);
        assert_eq!(used(&mem, RX, 0), (0, 4));
        assert_eq!(dev.device().pending_input(), 4);
        assert_eq!(dev.input(b"abcdef"), 4, "room freed by delivery");
        assert_eq!(dev.device().pending_input(), 8);
    }

    #[test]
    fn empty_receive_buffer_is_returned_with_length_zero() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        post_rx(&mem, 0, 0);
        post_rx(&mem, 1, 16);
        assert_eq!(dev.input(b"data"), 4);
        assert_eq!(used_idx(&mem, RX), 2);
        assert_eq!(used(&mem, RX, 0), (0, 0), "nothing fits in it");
        assert_eq!(used(&mem, RX, 1), (1, 4));
        assert_eq!(bytes(&mem, buf(1), 4), b"data");
    }

    #[test]
    fn malformed_receive_chains_need_reset_and_keep_the_input() {
        type Setup = fn(&VecMemory);
        let cases: [(&str, Setup, QueueError); 7] = [
            (
                "second buffer of the chain outside guest RAM",
                |m| {
                    desc(
                        m,
                        RX,
                        0,
                        buf(0),
                        2,
                        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
                        1,
                    );
                    desc(m, RX, 1, 0xFFFF_0000, 16, VIRTQ_DESC_F_WRITE, 0);
                },
                // The first two bytes were written, but none are lost.
                QueueError::Memory(aegis_core::GuestAccessError {
                    gpa: 0xFFFF_0000,
                    len: 2,
                }),
            ),
            (
                "device-readable buffer",
                |m| desc(m, RX, 0, buf(0), 16, 0, 0),
                QueueError::NotWritable,
            ),
            (
                "readable buffer after a writable one",
                |m| {
                    desc(
                        m,
                        RX,
                        0,
                        buf(0),
                        4,
                        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
                        1,
                    );
                    desc(m, RX, 1, buf(1), 4, 0, 0);
                },
                QueueError::NotWritable,
            ),
            (
                "buffer outside guest RAM",
                |m| desc(m, RX, 0, 0xFFFF_0000, 16, VIRTQ_DESC_F_WRITE, 0),
                QueueError::Memory(aegis_core::GuestAccessError {
                    gpa: 0xFFFF_0000,
                    len: 4,
                }),
            ),
            (
                "buffer wrapping the address space",
                |m| desc(m, RX, 0, u64::MAX - 7, 16, VIRTQ_DESC_F_WRITE, 0),
                QueueError::AddressOverflow,
            ),
            (
                "length above the chain cap",
                |m| desc(m, RX, 0, buf(0), u32::MAX, VIRTQ_DESC_F_WRITE, 0),
                QueueError::ChainTooLarge,
            ),
            (
                "next index out of range",
                |m| {
                    desc(
                        m,
                        RX,
                        0,
                        buf(0),
                        4,
                        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
                        99,
                    );
                },
                QueueError::BadIndex(99),
            ),
        ];
        for (what, setup_chain, expected) in cases {
            let (mem, line, console) = setup(VirtioConsole::default());
            let mut dev = VirtioMmio::new(console, &mem, &line);
            init_open(&mut dev, &mem);
            setup_chain(&mem);
            post(&mem, RX, 0, 0, 1);
            assert_eq!(dev.input(b"keep"), 4, "{what}");
            assert_eq!(dev.last_error(), Some(expected), "{what}");
            assert_ne!(dev.status() & status::DEVICE_NEEDS_RESET, 0, "{what}");
            assert_eq!(
                r(&mut dev, INTERRUPT_STATUS),
                u64::from(INT_CONFIG),
                "{what}"
            );
            assert_eq!(used_idx(&mem, RX), 0, "{what}: nothing returned");
            assert_eq!(dev.device().pending_input(), 4, "{what}: input kept");
            assert_eq!(dev.device().delivered_input(), 0, "{what}");
        }
    }

    #[test]
    fn receive_chain_at_the_size_cap_is_accepted_and_one_byte_more_is_not() {
        let mem = VecMemory::new(0x30_0000);
        let line = RecordingLine::default();
        let mut dev = VirtioMmio::new(VirtioConsole::default(), &mem, &line);
        init_open(&mut dev, &mem);
        let cap = u32::try_from(MAX_CHAIN_BYTES).unwrap();
        desc(&mem, RX, 0, 0x10_0000, cap, VIRTQ_DESC_F_WRITE, 0);
        post(&mem, RX, 0, 0, 1);
        assert_eq!(dev.input(b"edge"), 4);
        assert_eq!(used(&mem, RX, 0), (0, 4));
        assert_eq!(dev.last_error(), None);

        desc(
            &mem,
            RX,
            1,
            0x10_0000,
            cap,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            2,
        );
        desc(&mem, RX, 2, 0x20_0000, 1, VIRTQ_DESC_F_WRITE, 0);
        post(&mem, RX, 1, 1, 2);
        dev.input(b"over");
        assert_eq!(dev.last_error(), Some(QueueError::ChainTooLarge));
        assert_eq!(used_idx(&mem, RX), 1);
    }

    #[test]
    fn no_interrupt_flag_on_the_receive_queue_suppresses_the_interrupt() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        mem.write(RX.avail, &1u16.to_le_bytes()).unwrap(); // VIRTQ_AVAIL_F_NO_INTERRUPT
        post_rx(&mem, 0, 16);
        assert_eq!(dev.input(b"q"), 1);
        assert_eq!(used(&mem, RX, 0), (0, 1), "buffer still returned");
        assert_eq!(r(&mut dev, INTERRUPT_STATUS), 0);
        assert!(!line.asserted());
    }

    #[test]
    fn reset_and_reconfiguration_keep_undelivered_input() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        post_rx(&mem, 0, 2);
        assert_eq!(dev.input(b"abcd"), 4);
        assert_eq!(used(&mem, RX, 0), (0, 2));
        assert_eq!(dev.device().pending_input(), 2);

        w(&mut dev, STATUS, 0);
        assert_eq!(dev.status(), 0);
        assert!(!line.asserted(), "reset deasserts the line");
        assert!(!dev.device().port_open(), "reset closes the port");
        handshake(&mut dev);
        configure(&mut dev, 0, RX2);
        configure(&mut dev, 1, TX);
        driver_ok(&mut dev);
        desc(&mem, RX2, 0, buf(8), 16, VIRTQ_DESC_F_WRITE, 0);
        post(&mem, RX2, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 0);
        assert_eq!(
            used_idx(&mem, RX2),
            0,
            "closed until the guest writes again"
        );
        open_port(&mut dev, &mem);
        assert_eq!(used_idx(&mem, RX2), 1, "the new ring starts from 0");
        assert_eq!(used(&mem, RX2, 0), (0, 2));
        assert_eq!(bytes(&mem, buf(8), 2), b"cd");
        assert_eq!(used_idx(&mem, RX), 1, "the old ring is not touched");
        assert_eq!(dev.device().delivered_input(), 4);
    }

    #[test]
    fn host_processing_is_ignored_unless_the_device_and_queue_are_live() {
        // Before DRIVER_OK.
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        handshake(&mut dev);
        configure(&mut dev, 0, RX);
        post_rx(&mem, 0, 16);
        dev.input(b"x");
        assert_eq!(used_idx(&mem, RX), 0, "before DRIVER_OK");

        // Receive queue never enabled.
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        handshake(&mut dev);
        configure(&mut dev, 1, TX);
        driver_ok(&mut dev);
        open_port(&mut dev, &mem);
        post_rx(&mem, 0, 16);
        dev.input(b"y");
        assert_eq!(used_idx(&mem, RX), 0, "queue not ready");
        assert_eq!(dev.device().pending_input(), 1);

        // After DEVICE_NEEDS_RESET.
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init_open(&mut dev, &mem);
        desc(&mem, RX, 0, buf(0), 16, 0, 0); // readable: malformed
        post(&mem, RX, 0, 0, 1);
        dev.input(b"z");
        assert_ne!(dev.status() & status::DEVICE_NEEDS_RESET, 0);
        post_rx(&mem, 1, 16);
        dev.input(b"w");
        assert_eq!(used_idx(&mem, RX), 0, "a device needing reset does nothing");
        assert_eq!(dev.device().pending_input(), 2);
    }

    #[test]
    fn output_is_unchanged_while_input_waits() {
        let (mem, line, console) = setup(VirtioConsole::default());
        let mut dev = VirtioMmio::new(console, &mem, &line);
        init(&mut dev);
        assert_eq!(dev.input(b"pending"), 7);
        mem.write(buf(4), b"out").unwrap();
        desc(&mem, TX, 0, buf(4), 3, 0, 0);
        post(&mem, TX, 0, 0, 1);
        w(&mut dev, QUEUE_NOTIFY, 1);
        assert!(dev.device().port_open());
        assert_eq!(dev.device().output(), b"out");
        assert_eq!(used(&mem, TX, 0), (0, 0));
        assert_eq!(dev.device().pending_input(), 7, "input untouched");
        assert_eq!(used_idx(&mem, RX), 0);
    }
}
