//! Guest console output streamed to the host terminal as the guest produces
//! it (`aegis run`).
//!
//! Devices write through a [`Sink`] into one bounded [`OutputStream`], in
//! arrival order. A single writer thread runs [`OutputStream::drain_into`]:
//! it sleeps on a condition variable until output arrives (no polling),
//! takes everything queued, renders it, and flushes after each batch, so a
//! partial line such as a shell prompt shows at once.
//!
//! Rendering:
//! - Serial (`ttyS0`) output is written as it is.
//! - hvc0 output is written with an `hvc0| ` prefix on each line. An hvc0
//!   line cut short by serial output resumes on a new line prefixed
//!   `hvc0+ `, so the hvc0 text can be recovered exactly (except whether its
//!   very last line ended with a newline).
//! - Control bytes are escaped: printable ASCII, `\n` and `\t` stay, the
//!   `\r` of `\r\n` is dropped (even when the two arrive in different
//!   chunks), and anything else shows as `\xNN`, so a guest cannot send
//!   escape sequences to the host terminal.
//!
//! Bounds: at most the stream's capacity waits in the queue. Output that does
//! not fit, or that arrives after [`OutputStream::close`], is dropped and
//! counted. A device is never made to wait for the terminal, because it
//! writes on a vCPU thread with the device bus locked. If writing fails
//! (stdout closed), the writer stops writing but keeps draining, so devices
//! still never wait, and the bytes not shown are counted.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use aegis_core::OutputSink;

/// Bytes the stream holds before it starts dropping output.
pub(crate) const CAPACITY: usize = 1024 * 1024;

/// Which guest console output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Console {
    /// The 16550 UART (`ttyS0`).
    Serial,
    /// The virtio-console (`hvc0`).
    Hvc0,
}

impl Console {
    const ALL: [Self; 2] = [Self::Serial, Self::Hvc0];

    const fn index(self) -> usize {
        match self {
            Self::Serial => 0,
            Self::Hvc0 => 1,
        }
    }

    /// The prefix for a new line, or for a line resumed after a cut.
    const fn tag(self, resumed: bool) -> &'static str {
        match (self, resumed) {
            (Self::Serial, _) => "",
            (Self::Hvc0, false) => "hvc0| ",
            (Self::Hvc0, true) => "hvc0+ ",
        }
    }
}

/// What the stream did over a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamStats {
    /// Bytes each console produced, by [`Console`] index.
    pub(crate) received: [u64; 2],
    /// Bytes dropped because the queue was full (or closed), by console.
    pub(crate) dropped: [u64; 2],
    /// Bytes queued but not shown because writing failed.
    pub(crate) unwritten: u64,
    /// Why writing stopped, if it did.
    pub(crate) write_error: Option<String>,
}

#[derive(Default)]
struct Queue {
    chunks: VecDeque<(Console, Vec<u8>)>,
    queued: usize,
    received: [u64; 2],
    dropped: [u64; 2],
    closed: bool,
}

/// A bounded queue of guest output between the devices and the writer.
pub(crate) struct OutputStream {
    queue: Mutex<Queue>,
    ready: Condvar,
    capacity: usize,
}

fn len64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl OutputStream {
    /// A stream that holds at most `capacity` bytes waiting to be written.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        // Nothing in the queue can be left half-updated by a panic.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A sink that feeds `console`'s output into this stream.
    pub(crate) const fn sink(&self, console: Console) -> Sink<'_> {
        Sink {
            stream: self,
            console,
        }
    }

    /// Queue `bytes` from `console`, or as much as fits; drop and count the
    /// rest. Never waits for the writer.
    fn push(&self, console: Console, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let i = console.index();
        let mut q = self.lock();
        q.received[i] = q.received[i].saturating_add(len64(bytes.len()));
        let room = if q.closed {
            0
        } else {
            self.capacity.saturating_sub(q.queued)
        };
        let take = bytes.len().min(room);
        q.dropped[i] = q.dropped[i].saturating_add(len64(bytes.len().saturating_sub(take)));
        if take == 0 {
            return;
        }
        // Consecutive output from one console joins the last chunk, so a
        // UART's byte-at-a-time output costs no allocation per byte.
        match q.chunks.back_mut() {
            Some((last, data)) if *last == console => data.extend_from_slice(&bytes[..take]),
            _ => q.chunks.push_back((console, bytes[..take].to_vec())),
        }
        q.queued = q.queued.saturating_add(take);
        drop(q);
        self.ready.notify_one();
    }

    /// No more output will be pushed: the writer finishes what is queued and
    /// returns. Later pushes are dropped and counted.
    pub(crate) fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// The writer: render everything pushed, in order, to `out`, flushing
    /// after each batch, until the stream is closed and empty.
    pub(crate) fn drain_into(&self, out: &mut dyn Write) -> StreamStats {
        let mut renderer = Renderer::default();
        let (mut unwritten, mut write_error) = (0u64, None);
        loop {
            let (batch, closed) = {
                let mut q = self.lock();
                while q.chunks.is_empty() && !q.closed {
                    q = self.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
                }
                q.queued = 0;
                (std::mem::take(&mut q.chunks), q.closed)
            };
            let mut text = String::new();
            for (console, bytes) in &batch {
                renderer.render(*console, bytes, &mut text);
            }
            if closed {
                renderer.finish(&mut text);
            }
            if write_error.is_none()
                && let Err(e) = out.write_all(text.as_bytes()).and_then(|()| out.flush())
            {
                write_error = Some(e.to_string());
            }
            if write_error.is_some() {
                // Keep draining so devices never wait; count what was lost.
                let lost: usize = batch.iter().map(|(_, b)| b.len()).sum();
                unwritten = unwritten.saturating_add(len64(lost));
            }
            if closed {
                break;
            }
        }
        let q = self.lock();
        StreamStats {
            received: q.received,
            dropped: q.dropped,
            unwritten,
            write_error,
        }
    }
}

/// An [`OutputSink`] for one console of an [`OutputStream`].
pub(crate) struct Sink<'a> {
    stream: &'a OutputStream,
    console: Console,
}

impl OutputSink for Sink<'_> {
    fn output(&mut self, bytes: &[u8]) {
        self.stream.push(self.console, bytes);
    }
}

/// Turns the interleaved console streams into terminal text.
#[derive(Default)]
struct Renderer {
    /// The console whose line is open (the cursor is in the middle of it).
    open: Option<Console>,
    /// Per console: its last line was cut by the other console.
    cut: [bool; 2],
    /// Per console: a `\r` held back to see whether `\n` follows.
    pending_cr: [bool; 2],
}

impl Renderer {
    fn render(&mut self, console: Console, bytes: &[u8], out: &mut String) {
        let i = console.index();
        for &b in bytes {
            if self.pending_cr[i] {
                self.pending_cr[i] = false;
                if b == b'\n' {
                    self.piece(console, "\n", out);
                    continue;
                }
                self.piece(console, "\\x0d", out);
            }
            match b {
                b'\r' => self.pending_cr[i] = true,
                b'\n' => self.piece(console, "\n", out),
                b'\t' | 0x20..=0x7E => {
                    let mut c = [0u8; 4];
                    self.piece(console, char::from(b).encode_utf8(&mut c), out);
                }
                _ => {
                    let mut escaped = String::new();
                    let _ = write!(escaped, "\\x{b:02x}");
                    self.piece(console, &escaped, out);
                }
            }
        }
    }

    /// Write `piece` (one character, one escape, or a newline) as `console`,
    /// starting a tagged line first if `console`'s line is not the open one.
    fn piece(&mut self, console: Console, piece: &str, out: &mut String) {
        let i = console.index();
        if self.open != Some(console) {
            if let Some(other) = self.open {
                // End the other console's line where it stands; it resumes
                // on a line of its own.
                out.push('\n');
                self.cut[other.index()] = true;
            }
            out.push_str(console.tag(self.cut[i]));
            self.cut[i] = false;
            self.open = Some(console);
        }
        out.push_str(piece);
        if piece == "\n" {
            self.open = None;
        }
    }

    /// The end of the run: show held-back `\r`s and end an open line.
    fn finish(&mut self, out: &mut String) {
        for console in Console::ALL {
            if std::mem::take(&mut self.pending_cr[console.index()]) {
                self.piece(console, "\\x0d", out);
            }
        }
        if self.open.take().is_some() {
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The hvc0 text recovered from rendered output (as the live tests do):
    /// tagged lines in order, with the newline before an `hvc0+ ` line
    /// (a cut, not the guest's) removed.
    fn hvc0_text(rendered: &str) -> String {
        let mut text = String::new();
        for line in rendered.split_inclusive('\n') {
            if let Some(rest) = line.strip_prefix("hvc0| ") {
                text.push_str(rest);
            } else if let Some(rest) = line.strip_prefix("hvc0+ ") {
                if text.ends_with('\n') {
                    text.pop();
                }
                text.push_str(rest);
            }
        }
        text
    }

    fn render(chunks: &[(Console, &[u8])]) -> String {
        let mut r = Renderer::default();
        let mut out = String::new();
        for (console, bytes) in chunks {
            r.render(*console, bytes, &mut out);
        }
        r.finish(&mut out);
        out
    }

    use Console::{Hvc0, Serial};

    #[test]
    fn serial_is_written_as_is_and_hvc0_lines_are_tagged() {
        assert_eq!(
            render(&[(Serial, b"Linux version 6\nok\n")]),
            "Linux version 6\nok\n"
        );
        assert_eq!(
            render(&[(Hvc0, b"READY\nline two\n")]),
            "hvc0| READY\nhvc0| line two\n"
        );
        assert_eq!(
            render(&[(Hvc0, b"a\n\nb\n")]),
            "hvc0| a\nhvc0| \nhvc0| b\n",
            "empty line"
        );
    }

    #[test]
    fn a_cut_line_resumes_with_a_continuation_tag_and_recovers_exactly() {
        let out = render(&[
            (Hvc0, b"AEGIS_"),
            (Serial, b"[ 1.0] kernel says hi\n"),
            (Hvc0, b"RX[x]\nnext\n"),
            (Serial, b"partial"),
            (Hvc0, b"tail\n"),
            (Serial, b" line\n"),
        ]);
        assert_eq!(
            out,
            "hvc0| AEGIS_\n[ 1.0] kernel says hi\nhvc0+ RX[x]\nhvc0| next\npartial\nhvc0| tail\n line\n"
        );
        assert_eq!(hvc0_text(&out), "AEGIS_RX[x]\nnext\ntail\n");
    }

    #[test]
    fn control_bytes_are_escaped_even_across_chunks() {
        assert_eq!(
            render(&[(Serial, b"ok\r\nnext\tline\n")]),
            "ok\nnext\tline\n"
        );
        assert_eq!(
            render(&[(Serial, b"\x1b[2Jboom\x07\n")]),
            "\\x1b[2Jboom\\x07\n"
        );
        assert_eq!(
            render(&[(Serial, b"split\r"), (Serial, b"\nlone\rcr\n")]),
            "split\nlone\\x0dcr\n",
            "\\r\\n across two chunks, and a lone \\r"
        );
        assert_eq!(
            render(&[(Serial, b"end\r")]),
            "end\\x0d\n",
            "a \\r held at the end"
        );
        assert_eq!(
            render(&[(Hvc0, &[0xFF, 0x00, b'\n'])]),
            "hvc0| \\xff\\x00\n"
        );
    }

    #[test]
    fn output_is_bounded_and_the_excess_counted_per_console() {
        let stream = OutputStream::new(8);
        stream.sink(Serial).output(b"12345");
        stream.sink(Hvc0).output(b"abcdef");
        stream.sink(Serial).output(b"");
        stream.close();
        stream.sink(Serial).output(b"late");
        let mut out = Vec::new();
        let stats = stream.drain_into(&mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "12345\nhvc0| abc\n");
        assert_eq!(stats.received, [9, 6]);
        assert_eq!(
            stats.dropped,
            [4, 3],
            "3 hvc0 bytes over capacity, 4 serial after close"
        );
        assert_eq!((stats.unwritten, stats.write_error), (0, None));
    }

    #[test]
    fn consecutive_output_from_one_console_shares_a_chunk() {
        let stream = OutputStream::new(1024);
        for &b in b"bytes one at a time" {
            stream.sink(Serial).output(&[b]);
        }
        stream.sink(Hvc0).output(b"x");
        stream.sink(Serial).output(b"!");
        assert_eq!(stream.lock().chunks.len(), 3, "serial, hvc0, serial");
    }

    /// A writer that fails after a number of successful writes.
    struct Failing(usize);

    impl Write for Failing {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdout closed",
                ));
            }
            self.0 = self.0.saturating_sub(1);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_write_stops_output_but_not_the_draining() {
        let stream = Arc::new(OutputStream::new(1 << 20));
        let writer = {
            let stream = Arc::clone(&stream);
            std::thread::spawn(move || stream.drain_into(&mut Failing(0)))
        };
        for _ in 0..100 {
            stream.sink(Serial).output(b"0123456789");
        }
        stream.close();
        let stats = writer.join().unwrap();
        assert_eq!(stats.write_error.as_deref(), Some("stdout closed"));
        assert_eq!(stats.unwritten, 1000, "every byte counted as not shown");
        assert_eq!(stats.received, [1000, 0]);
    }

    /// `count` lines: `<prefix>0`, `<prefix>1`, and so on.
    fn numbered(prefix: &str, count: usize) -> String {
        (0..count).fold(String::new(), |mut s, i| {
            let _ = writeln!(s, "{prefix}{i}");
            s
        })
    }

    #[test]
    fn concurrent_devices_keep_each_consoles_order() {
        let stream = Arc::new(OutputStream::new(1 << 20));
        let writer = {
            let stream = Arc::clone(&stream);
            std::thread::spawn(move || {
                let mut out = Vec::new();
                let stats = stream.drain_into(&mut out);
                (String::from_utf8(out).unwrap(), stats)
            })
        };
        let producers: Vec<_> = [(Serial, "s"), (Hvc0, "h")]
            .into_iter()
            .map(|(console, prefix)| {
                let stream = Arc::clone(&stream);
                std::thread::spawn(move || {
                    for i in 0..500 {
                        stream
                            .sink(console)
                            .output(format!("{prefix}{i}\n").as_bytes());
                    }
                })
            })
            .collect();
        for p in producers {
            p.join().unwrap();
        }
        stream.close();
        let (out, stats) = writer.join().unwrap();
        let expected_hvc = numbered("h", 500);
        assert_eq!(hvc0_text(&out), expected_hvc);
        let serial: String = out
            .split_inclusive('\n')
            .filter(|l| !l.starts_with("hvc0"))
            .collect();
        let expected_serial = numbered("s", 500);
        assert_eq!(serial, expected_serial);
        assert_eq!(stats.dropped, [0, 0]);
    }

    #[test]
    fn the_writer_returns_once_closed_even_with_nothing_queued() {
        let stream = Arc::new(OutputStream::new(16));
        let writer = {
            let stream = Arc::clone(&stream);
            std::thread::spawn(move || stream.drain_into(&mut Vec::new()))
        };
        stream.close();
        assert_eq!(writer.join().unwrap(), StreamStats::default());
    }
}
