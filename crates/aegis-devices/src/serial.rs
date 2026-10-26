//! A transmit-only 16550 UART: enough for a guest to print to a console.
//!
//! Registers (offsets from the base port, usually `0x3F8`):
//!
//! | Offset | DLAB=0 read | DLAB=0 write | DLAB=1 |
//! |---|---|---|---|
//! | 0 | RBR: always 0 (no input) | THR: byte appended to output | DLL |
//! | 1 | IER | IER | DLM |
//! | 2 | IIR: 0x01 (no interrupt pending) | FCR (ignored) | |
//! | 3 | LCR | LCR (bit 7 = DLAB) | |
//! | 4 | MCR | MCR | |
//! | 5 | LSR: 0x60 (transmitter empty) | ignored | |
//! | 6 | MSR: 0 | ignored | |
//! | 7 | SCR | SCR | |
//!
//! Not modeled: receive, interrupts, FIFOs, baud timing.
//!
//! Transmitted bytes go to the UART's [`OutputSink`], one byte per THR
//! write. The default sink, [`CappedBuffer`], keeps at most
//! [`Serial16550::DEFAULT_CAPACITY`] bytes and counts the excess, so a guest
//! cannot exhaust host memory through the console
//! (`research/security-analysis.md` §3.4). A host that streams the console
//! passes its own sink to [`Serial16550::with_sink`]. Bytes are raw: escape
//! them before writing them to a terminal.

use aegis_core::{AccessSize, OutputSink, PortDevice};

use crate::output::CappedBuffer;

/// LSR: transmit holding register empty (bit 5) and transmitter empty (bit 6).
const LSR_TX_EMPTY: u32 = 0x60;
/// IIR: no interrupt pending.
const IIR_NONE: u32 = 0x01;
/// LCR bit 7: divisor latch access.
const LCR_DLAB: u8 = 0x80;

/// A transmit-only 16550 UART whose output goes to `S`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serial16550<S = CappedBuffer> {
    sink: S,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    divisor: [u8; 2],
}

impl Default for Serial16550 {
    fn default() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }
}

impl Serial16550 {
    /// Number of ports the device occupies.
    pub const PORTS: u16 = 8;
    /// Default output cap in bytes.
    pub const DEFAULT_CAPACITY: usize = 64 * 1024;

    /// A UART that keeps at most `capacity` output bytes.
    #[must_use]
    pub const fn with_capacity(capacity: usize) -> Self {
        Self::with_sink(CappedBuffer::new(capacity))
    }

    /// Bytes the guest has transmitted (up to the cap).
    #[must_use]
    pub fn output(&self) -> &[u8] {
        self.sink.bytes()
    }

    /// Bytes dropped because the cap was reached.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.sink.dropped()
    }
}

impl<S> Serial16550<S> {
    /// A UART whose transmitted bytes go to `sink`.
    pub const fn with_sink(sink: S) -> Self {
        Self {
            sink,
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            divisor: [0; 2],
        }
    }

    /// The output sink.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// The divisor latch, as last programmed by the guest.
    #[must_use]
    pub const fn divisor(&self) -> u16 {
        u16::from_le_bytes(self.divisor)
    }

    const fn dlab(&self) -> bool {
        self.lcr & LCR_DLAB != 0
    }
}

impl<S: OutputSink> PortDevice for Serial16550<S> {
    fn read(&mut self, offset: u16, _size: AccessSize) -> u32 {
        let value = match (offset, self.dlab()) {
            (0, true) => self.divisor[0],
            (1, true) => self.divisor[1],
            (1, false) => self.ier,
            (2, _) => return IIR_NONE,
            (3, _) => self.lcr,
            (4, _) => self.mcr,
            (5, _) => return LSR_TX_EMPTY,
            (7, _) => self.scr,
            // RBR with no input, MSR, and anything past the 8 ports.
            _ => 0,
        };
        u32::from(value)
    }

    fn write(&mut self, offset: u16, _size: AccessSize, value: u32) {
        // A 16550 register is 8 bits wide; wider accesses use the low byte.
        let byte = value.to_le_bytes()[0];
        match (offset, self.dlab()) {
            (0, true) => self.divisor[0] = byte,
            (1, true) => self.divisor[1] = byte,
            (0, false) => self.sink.output(&[byte]),
            (1, false) => self.ier = byte,
            (3, _) => self.lcr = byte,
            (4, _) => self.mcr = byte,
            (7, _) => self.scr = byte,
            // FCR, LSR, MSR and out-of-range offsets: ignored.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: AccessSize = AccessSize::Byte;

    #[test]
    fn transmits_thr_writes_and_reports_an_empty_transmitter() {
        let mut uart = Serial16550::default();
        for &b in b"ok\n" {
            assert_eq!(uart.read(5, B) & LSR_TX_EMPTY, LSR_TX_EMPTY);
            uart.write(0, B, u32::from(b));
        }
        assert_eq!(uart.output(), b"ok\n");
        assert_eq!(uart.read(2, B), IIR_NONE);
        assert_eq!(uart.read(0, B), 0, "no receive data");
    }

    #[test]
    fn dlab_redirects_offsets_0_and_1_to_the_divisor() {
        let mut uart = Serial16550::default();
        uart.write(3, B, 0x83); // DLAB=1, 8N1
        uart.write(0, B, 0x01); // 115200 baud: divisor 1
        uart.write(1, B, 0x00);
        assert_eq!(uart.divisor(), 1);
        assert_eq!(uart.read(0, B), 1);
        uart.write(3, B, 0x03); // DLAB=0
        uart.write(1, B, 0x0F);
        assert_eq!(uart.read(1, B), 0x0F, "IER, not DLM");
        assert!(uart.output().is_empty(), "divisor writes are not output");
    }

    #[test]
    fn scratch_and_control_registers_read_back() {
        let mut uart = Serial16550::default();
        uart.write(7, B, 0xA5);
        uart.write(4, B, 0x0B);
        assert_eq!(uart.read(7, B), 0xA5);
        assert_eq!(uart.read(4, B), 0x0B);
        assert_eq!(uart.read(6, B), 0);
    }

    #[test]
    fn output_is_capped_and_overflow_counted() {
        let mut uart = Serial16550::with_capacity(4);
        for b in 0..10u32 {
            uart.write(0, B, b);
        }
        assert_eq!(uart.output(), [0, 1, 2, 3]);
        assert_eq!(uart.dropped(), 6);
    }

    /// A sink that records each call.
    #[derive(Default)]
    struct Calls(alloc::vec::Vec<alloc::vec::Vec<u8>>);

    impl OutputSink for Calls {
        fn output(&mut self, bytes: &[u8]) {
            self.0.push(bytes.to_vec());
        }
    }

    #[test]
    fn a_custom_sink_gets_each_byte_as_it_is_transmitted() {
        let mut uart = Serial16550::with_sink(Calls::default());
        uart.write(0, B, u32::from(b'h'));
        uart.write(3, B, 0x80); // DLAB=1: the next write is the divisor, not output
        uart.write(0, B, 1);
        uart.write(3, B, 0x03);
        uart.write(0, B, u32::from(b'i'));
        assert_eq!(uart.sink().0, [b"h".to_vec(), b"i".to_vec()]);
    }

    #[test]
    fn wide_writes_use_the_low_byte_and_bad_offsets_are_ignored() {
        let mut uart = Serial16550::default();
        uart.write(0, AccessSize::Dword, 0xDEAD_BE41);
        uart.write(9, B, 0x42);
        assert_eq!(uart.output(), b"A");
        assert_eq!(uart.read(9, B), 0);
    }
}
