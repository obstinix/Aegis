//! Host input for a guest device: forwarding a file descriptor, normally
//! stdin, to a device such as the virtio-console.
//!
//! [`forward`] runs on its own host thread. It never blocks indefinitely:
//! it waits for input with `poll` and a [`POLL_INTERVAL`] timeout, and
//! checks the stop flag between waits. It reads only after `poll` reports
//! the descriptor readable (or at end of file). It returns on end of file,
//! on an error, or within about one interval of the flag being set. One case
//! remains: another process sharing the descriptor drains it between `poll`
//! and `read`, so `read` blocks. Registering the thread as a
//! [`crate::smp::HostThread`] covers that: the SMP runner's `SIGRTMIN` kick
//! makes the `read` return `EINTR`.
//!
//! The device may not take everything at once: its input buffer is bounded,
//! and it fills only as fast as the guest posts receive buffers. Bytes the
//! device does not take stay in the worker's one-chunk buffer and are
//! offered again every interval, and nothing more is read meanwhile. That
//! is backpressure, not loss. Memory use is fixed at one [`CHUNK`].
//!
//! After the stop flag is set, nothing more is delivered, so no input
//! reaches a VM that has stopped.

use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The standard input file descriptor.
pub const STDIN: RawFd = 0;
/// Longest wait between stop-flag checks.
pub const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Most bytes read (and held) at once.
pub const CHUNK: usize = 1024;

/// Why [`forward`] returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEnd {
    /// The descriptor reached end of file, and everything read was delivered.
    Eof,
    /// The stop flag was set.
    Stopped,
    /// Reading or polling failed, or the worker could not be sandboxed.
    Error(String),
}

/// What [`forward`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputReport {
    /// Bytes the device took.
    pub forwarded: u64,
    /// Bytes read but not delivered because the run stopped first.
    pub undelivered: usize,
    /// Why it returned.
    pub end: InputEnd,
}

impl InputReport {
    /// A report for a worker that never started forwarding.
    #[must_use]
    pub const fn failed(reason: String) -> Self {
        Self {
            forwarded: 0,
            undelivered: 0,
            end: InputEnd::Error(reason),
        }
    }
}

#[allow(unsafe_code)]
fn poll_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: `pfd` is an initialized pollfd that lives for the whole call,
    // and nfds = 1 matches the single entry. poll only writes `revents`.
    let ready = unsafe { libc::poll(&raw mut pfd, 1, ms) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    if pfd.revents & libc::POLLNVAL != 0 {
        return Err(io::Error::from_raw_os_error(libc::EBADF));
    }
    // POLLHUP and POLLERR also mean read will not block: it returns end of
    // file or the error.
    Ok(ready > 0 && pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
}

#[allow(unsafe_code)]
fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buf` is valid for writes of `buf.len()` bytes for the whole
    // call, and read writes at most that many.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    usize::try_from(n).map_err(|_| io::Error::last_os_error())
}

/// Read `fd` and hand its bytes to `deliver` until end of file, an error, or
/// `stop`. `deliver` returns how many leading bytes it took (at most the
/// slice length); the rest is offered again later.
pub fn forward(
    fd: RawFd,
    stop: &AtomicBool,
    deliver: &mut dyn FnMut(&[u8]) -> usize,
) -> InputReport {
    let mut buf = [0u8; CHUNK];
    let (mut start, mut end) = (0usize, 0usize);
    let mut forwarded: u64 = 0;
    let mut eof = false;
    let outcome = loop {
        if stop.load(Ordering::Acquire) {
            break InputEnd::Stopped;
        }
        if start < end {
            let pending = &buf[start..end];
            let taken = deliver(pending).min(pending.len());
            start = start.saturating_add(taken);
            forwarded = forwarded.saturating_add(u64::try_from(taken).unwrap_or(u64::MAX));
            if start < end {
                // The device is full until the guest reads.
                std::thread::sleep(POLL_INTERVAL);
            }
            continue;
        }
        if eof {
            break InputEnd::Eof;
        }
        match poll_readable(fd, POLL_INTERVAL).and_then(|readable| {
            if readable {
                read_fd(fd, &mut buf).map(Some)
            } else {
                Ok(None)
            }
        }) {
            Ok(None) => {}
            Ok(Some(0)) => eof = true,
            Ok(Some(n)) => (start, end) = (0, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => break InputEnd::Error(e.to_string()),
        }
    };
    InputReport {
        forwarded,
        undelivered: end.saturating_sub(start),
        end: outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    #[test]
    fn forwards_everything_in_order_until_end_of_file() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let input: Vec<u8> = (0..5000u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        writer.write_all(&input).unwrap();
        drop(writer);
        let mut got = Vec::new();
        let stop = AtomicBool::new(false);
        let report = forward(reader.as_raw_fd(), &stop, &mut |b| {
            got.extend_from_slice(b);
            b.len()
        });
        assert_eq!(report.end, InputEnd::Eof);
        assert_eq!(report.forwarded, 5000);
        assert_eq!(report.undelivered, 0);
        assert_eq!(got, input, "more than one chunk, in order");
    }

    #[test]
    fn a_full_device_gets_the_rest_later_and_nothing_is_lost() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"backpressure").unwrap();
        drop(writer);
        let mut got = Vec::new();
        let mut calls = 0;
        let stop = AtomicBool::new(false);
        let report = forward(reader.as_raw_fd(), &stop, &mut |b| {
            calls += 1;
            // Take one byte on every other call, as a device that fills up.
            if calls % 2 == 0 {
                got.push(b[0]);
                1
            } else {
                0
            }
        });
        assert_eq!(report.end, InputEnd::Eof);
        assert_eq!(got, b"backpressure");
        assert_eq!(report.forwarded, 12);
    }

    #[test]
    fn stops_within_a_few_intervals_with_no_input() {
        let (_writer, reader) = UnixStream::pair().unwrap();
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let report = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(60));
                stop.store(true, Ordering::Release);
            });
            forward(reader.as_raw_fd(), &stop, &mut |b| b.len())
        });
        assert_eq!(report.end, InputEnd::Stopped);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "returned {:?} after the stop request",
            started.elapsed()
        );
    }

    #[test]
    fn nothing_is_delivered_once_stopped() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"too late").unwrap();
        let stop = AtomicBool::new(true);
        let report = forward(reader.as_raw_fd(), &stop, &mut |_| {
            panic!("delivered after stop");
        });
        assert_eq!(report.end, InputEnd::Stopped);
        assert_eq!(report.forwarded, 0);
    }

    #[test]
    fn input_held_when_the_run_stops_is_reported_undelivered() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"stuck").unwrap();
        let stop = AtomicBool::new(false);
        let report = forward(reader.as_raw_fd(), &stop, &mut |_| {
            // The guest never reads; the run then stops.
            stop.store(true, Ordering::Release);
            0
        });
        assert_eq!(report.end, InputEnd::Stopped);
        assert_eq!(report.undelivered, 5);
    }

    #[test]
    fn an_invalid_descriptor_is_an_error_not_a_hang() {
        let stop = AtomicBool::new(false);
        let report = forward(RawFd::MAX, &stop, &mut |b| b.len());
        assert!(matches!(report.end, InputEnd::Error(_)), "{:?}", report.end);
    }
}
