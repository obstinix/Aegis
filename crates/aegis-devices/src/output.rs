//! The default console output sink.
//!
//! [`CappedBuffer`] keeps the first `capacity` bytes a device outputs and
//! counts the rest, so a guest cannot exhaust host memory through a console
//! (`research/security-analysis.md` §3.4). A host that streams output
//! elsewhere gives the device its own [`aegis_core::OutputSink`] instead.

use alloc::vec::Vec;

use aegis_core::OutputSink;

/// Guest output kept in memory, up to a cap. Bytes are stored raw: escape
/// them before writing them to a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedBuffer {
    bytes: Vec<u8>,
    capacity: usize,
    dropped: u64,
}

impl CappedBuffer {
    /// A buffer that keeps at most `capacity` bytes.
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::new(),
            capacity,
            dropped: 0,
        }
    }

    /// The bytes kept so far.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Bytes dropped because the cap was reached.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl OutputSink for CappedBuffer {
    fn output(&mut self, data: &[u8]) {
        let room = self.capacity.saturating_sub(self.bytes.len());
        let keep = data.len().min(room);
        self.bytes.extend_from_slice(&data[..keep]);
        let lost = u64::try_from(data.len().saturating_sub(keep)).unwrap_or(u64::MAX);
        self.dropped = self.dropped.saturating_add(lost);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_up_to_the_cap_and_counts_the_rest() {
        let mut buf = CappedBuffer::new(5);
        buf.output(b"abc");
        buf.output(b"defg");
        buf.output(b"");
        buf.output(b"h");
        assert_eq!(buf.bytes(), b"abcde");
        assert_eq!(buf.dropped(), 3);
        let mut none = CappedBuffer::new(0);
        none.output(b"xy");
        assert_eq!((none.bytes(), none.dropped()), (&b""[..], 2));
    }
}
