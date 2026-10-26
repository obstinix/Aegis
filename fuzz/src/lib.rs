//! Fuzz harnesses for Aegis (docs/fuzzing.md).
//!
//! Each harness takes arbitrary bytes, drives real Aegis code with them, and
//! panics if an invariant breaks. A panic is what the fuzzer reports as a
//! crash. The same functions back:
//! - the cargo-fuzz targets in `fuzz_targets/` (nightly, `--features libfuzzer`);
//! - the deterministic smoke and regression tests below, which run on stable;
//! - `examples/reproduce.rs`, which replays a saved input on stable.
//!
//! The harnesses return a small report of how far the input got (chains
//! popped, `DRIVER_OK` reached, and so on). The tests use it to check that
//! the generated inputs reach the interesting paths, not just the early exits.

#![forbid(unsafe_code)]

use std::path::Path;

use aegis_core::mock::{RecordingLine, VecMemory};
use aegis_core::{AccessSize, GuestMemoryAccess, MmioDevice};
use aegis_devices::VirtioConsole;
use aegis_devices::virtio::VirtioMmio;
use aegis_devices::virtio::mmio::status::{DEVICE_NEEDS_RESET, DRIVER_OK, FEATURES_OK};
use aegis_devices::virtio::queue::{MAX_CHAIN_BYTES, SplitQueue};

/// The `vm.toml` parser, included from `aegis-cli`, a binary crate this one
/// cannot depend on. It is compiled here exactly as it is there, tests
/// included.
#[allow(dead_code, unreachable_pub, clippy::all, clippy::pedantic)]
#[path = "../../crates/aegis-cli/src/config.rs"]
mod config;

/// Guest RAM for the device harnesses.
pub const MEMORY_BYTES: usize = 64 * 1024;
/// Largest queue size the virtqueue harness enables (the console's).
const QUEUE_MAX: u16 = 256;

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([
        data.get(at).copied().unwrap_or(0),
        data.get(at + 1).copied().unwrap_or(0),
    ])
}

/// Copy as much of `image` as fits into fresh guest RAM.
fn memory_from(image: &[u8]) -> VecMemory {
    let mem = VecMemory::new(MEMORY_BYTES);
    let len = image.len().min(MEMORY_BYTES);
    mem.write(0, &image[..len]).expect("within guest RAM");
    mem
}

/// How far a [`virtqueue`] input got.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QueueReport {
    /// The queue configuration was accepted.
    pub enabled: bool,
    /// Chains taken from the available ring.
    pub chains: usize,
}

/// Virtqueue processing.
///
/// Input: bytes 0..8 are the queue configuration (size, then the descriptor,
/// available and used ring addresses, each a little-endian `u16`). The rest
/// is guest RAM from address 0, so the fuzzer controls every ring and
/// descriptor. The harness enables the queue and pops chains, returning
/// each to the used ring, until the ring is empty or reports an error.
///
/// Invariants:
/// - a queue that enables has a power-of-two size up to the maximum and
///   correctly aligned rings;
/// - every popped chain has a head below the queue size, at least one and at
///   most `size` buffers, a total length within [`MAX_CHAIN_BYTES`], and no
///   buffer whose end wraps the address space;
/// - nothing panics.
///
/// # Panics
///
/// If an invariant breaks.
#[must_use]
pub fn virtqueue(data: &[u8]) -> QueueReport {
    let mut report = QueueReport::default();
    let mem = memory_from(data.get(8..).unwrap_or(&[]));
    let mut queue = SplitQueue::default();
    queue.size = u16_at(data, 0);
    queue.desc = u64::from(u16_at(data, 2));
    queue.avail = u64::from(u16_at(data, 4));
    queue.used = u64::from(u16_at(data, 6));
    if queue.enable(QUEUE_MAX).is_err() {
        assert!(!queue.is_ready());
        return report;
    }
    report.enabled = true;
    assert!(queue.size.is_power_of_two() && queue.size <= QUEUE_MAX);
    assert_eq!(queue.desc % 16, 0);
    assert_eq!(queue.avail % 2, 0);
    assert_eq!(queue.used % 4, 0);

    // At most one pass over the ring, plus room to see the overrun check.
    for _ in 0..=usize::from(queue.size) {
        let Ok(Some(chain)) = queue.pop(&mem) else {
            break;
        };
        report.chains += 1;
        assert!(
            chain.head < queue.size,
            "head {} of {}",
            chain.head,
            queue.size
        );
        assert!(!chain.buffers.is_empty());
        assert!(chain.buffers.len() <= usize::from(queue.size));
        let total: u64 = chain.buffers.iter().map(|b| u64::from(b.len)).sum();
        assert!(total <= MAX_CHAIN_BYTES, "{total} bytes in one chain");
        for b in &chain.buffers {
            assert!(
                b.addr.checked_add(u64::from(b.len)).is_some(),
                "{b:?} wraps"
            );
        }
        let written = u32::try_from(total).expect("within MAX_CHAIN_BYTES");
        if queue.push_used(&mem, chain.head, written).is_err() {
            break;
        }
        let _ = queue.wants_interrupt(&mem);
    }
    report
}

/// virtio-mmio register offsets the harness and its seeds use (virtio 1.2,
/// §4.2.2), written out as a guest driver sees them.
pub mod regs {
    /// `DriverFeatures`.
    pub const DRIVER_FEATURES: u16 = 0x020;
    /// `DriverFeaturesSel`.
    pub const DRIVER_FEATURES_SEL: u16 = 0x024;
    /// `QueueSel`.
    pub const QUEUE_SEL: u16 = 0x030;
    /// `QueueNum`.
    pub const QUEUE_NUM: u16 = 0x038;
    /// `QueueReady`.
    pub const QUEUE_READY: u16 = 0x044;
    /// `QueueNotify`.
    pub const QUEUE_NOTIFY: u16 = 0x050;
    /// `InterruptStatus`.
    pub const INTERRUPT_STATUS: u16 = 0x060;
    /// `InterruptACK`.
    pub const INTERRUPT_ACK: u16 = 0x064;
    /// `Status`.
    pub const STATUS: u16 = 0x070;
    /// `QueueDescLow`.
    pub const QUEUE_DESC_LOW: u16 = 0x080;
    /// `QueueDriverLow`.
    pub const QUEUE_DRIVER_LOW: u16 = 0x090;
    /// `QueueDeviceLow`.
    pub const QUEUE_DEVICE_LOW: u16 = 0x0A0;
}

/// One step of a [`virtio_mmio`] script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmioOp {
    /// A guest register write.
    Write {
        /// Register offset (below `0x200`: registers and configuration space).
        offset: u16,
        /// 1, 2, 4 or 8 bytes.
        size: AccessSize,
        /// Value written.
        value: u32,
    },
    /// A guest register read.
    Read {
        /// Register offset.
        offset: u16,
        /// Access width.
        size: AccessSize,
    },
    /// Host input offered to the console (up to 4 bytes).
    Input {
        /// Bytes offered, of which the first `len` count.
        bytes: [u8; 4],
        /// How many bytes, 0..=4.
        len: u8,
    },
    /// The host asks the transport to process queue `queue` (0..=2).
    Process {
        /// Queue index.
        queue: u8,
    },
}

const SIZES: [AccessSize; 4] = [
    AccessSize::Byte,
    AccessSize::Word,
    AccessSize::Dword,
    AccessSize::Qword,
];

fn size_index(size: AccessSize) -> u8 {
    match size {
        AccessSize::Byte => 0,
        AccessSize::Word => 1,
        AccessSize::Dword => 2,
        AccessSize::Qword => 3,
    }
}

impl MmioOp {
    /// Bytes per encoded step.
    pub const ENCODED: usize = 8;

    /// Decode one step. Every 8-byte pattern decodes to some step.
    #[must_use]
    pub fn decode(b: [u8; 8]) -> Self {
        let offset = u16::from_le_bytes([b[1], b[2]]) % 0x200;
        let size = SIZES[usize::from(b[3] % 4)];
        let value = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        match b[0] % 4 {
            0 => Self::Write {
                offset,
                size,
                value,
            },
            1 => Self::Read { offset, size },
            2 => Self::Input {
                bytes: [b[4], b[5], b[6], b[7]],
                len: b[3] % 5,
            },
            _ => Self::Process { queue: b[4] % 3 },
        }
    }

    /// Encode this step (the inverse of [`MmioOp::decode`] for steps whose
    /// fields are in range).
    #[must_use]
    pub fn encode(self) -> [u8; 8] {
        match self {
            Self::Write {
                offset,
                size,
                value,
            } => {
                let [o0, o1] = offset.to_le_bytes();
                let [v0, v1, v2, v3] = value.to_le_bytes();
                [0, o0, o1, size_index(size), v0, v1, v2, v3]
            }
            Self::Read { offset, size } => {
                let [o0, o1] = offset.to_le_bytes();
                [1, o0, o1, size_index(size), 0, 0, 0, 0]
            }
            Self::Input { bytes, len } => [2, 0, 0, len, bytes[0], bytes[1], bytes[2], bytes[3]],
            Self::Process { queue } => [3, 0, 0, 0, queue, 0, 0, 0],
        }
    }
}

/// Small caps, so the harness reaches the limits.
const OUTPUT_CAP: usize = 256;
const INPUT_CAP: usize = 64;

/// How far a [`virtio_mmio`] input got.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MmioReport {
    /// Steps run.
    pub steps: usize,
    /// `DRIVER_OK` was set at some point.
    pub driver_ok: bool,
    /// Bytes the guest transmitted (kept or dropped).
    pub output: u64,
    /// Input bytes written into receive buffers.
    pub delivered: u64,
    /// The device needed a reset at some point.
    pub needs_reset: bool,
}

/// virtio-mmio register handling, with the console behind it.
///
/// Input: bytes 0..2 are the number of script steps `n` (little-endian).
/// Then come up to `n` [`MmioOp`]s of [`MmioOp::ENCODED`] bytes each, and
/// the rest is guest RAM from address 0. The script comes first so that a
/// mutation of the (larger) RAM image does not shift the script.
///
/// Invariants, checked after every step:
/// - the status only gains bits, except that writing 0 resets the device
///   (status 0, no recorded error, interrupt line deasserted);
/// - `DEVICE_NEEDS_RESET` is set exactly when an error is recorded;
/// - `DRIVER_OK` implies `FEATURES_OK`;
/// - the interrupt line is asserted exactly when `InterruptStatus` is
///   non-zero;
/// - output stays within its cap, and pending input within the input cap;
/// - delivered input never decreases, and delivered plus pending never
///   exceeds the input accepted;
/// - nothing panics.
///
/// # Panics
///
/// If an invariant breaks.
#[must_use]
pub fn virtio_mmio(data: &[u8]) -> MmioReport {
    let body = data.get(2..).unwrap_or(&[]);
    let script_len = (usize::from(u16_at(data, 0)) * MmioOp::ENCODED).min(body.len());
    let (script, image) = body.split_at(script_len);
    let mem = memory_from(image);
    let line = RecordingLine::default();
    let console = VirtioConsole::with_limits(OUTPUT_CAP, INPUT_CAP);
    let mut dev = VirtioMmio::new(console, &mem, &line);
    let mut report = MmioReport::default();
    let (mut accepted, mut last_delivered) = (0u64, 0u64);

    for step in script.chunks_exact(MmioOp::ENCODED) {
        let op = MmioOp::decode(step.try_into().expect("an 8-byte chunk"));
        let before = dev.status();
        let mut reset = false;
        match op {
            MmioOp::Write {
                offset,
                size,
                value,
            } => {
                reset = offset == regs::STATUS && size == AccessSize::Dword && value == 0;
                dev.write(u64::from(offset), size, u64::from(value));
            }
            MmioOp::Read { offset, size } => {
                let _ = dev.read(u64::from(offset), size);
            }
            MmioOp::Input { bytes, len } => {
                let offered = &bytes[..usize::from(len)];
                let taken = dev.input(offered);
                assert!(taken <= offered.len());
                accepted += u64::try_from(taken).expect("at most 4");
            }
            MmioOp::Process { queue } => dev.process(usize::from(queue)),
        }
        report.steps += 1;

        let status = dev.status();
        if reset {
            assert_eq!(status, 0, "status {status:#x} after a reset");
            assert!(dev.last_error().is_none() && !line.asserted());
        } else {
            assert_eq!(
                status & before,
                before,
                "status {before:#x} lost bits: {status:#x}"
            );
        }
        assert_eq!(
            status & DEVICE_NEEDS_RESET != 0,
            dev.last_error().is_some(),
            "status {status:#x} with error {:?}",
            dev.last_error()
        );
        if status & DRIVER_OK != 0 {
            assert_ne!(status & FEATURES_OK, 0, "DRIVER_OK without FEATURES_OK");
            report.driver_ok = true;
        }
        report.needs_reset |= status & DEVICE_NEEDS_RESET != 0;
        let pending_interrupts = dev.read(u64::from(regs::INTERRUPT_STATUS), AccessSize::Dword);
        assert_eq!(
            line.asserted(),
            pending_interrupts != 0,
            "line {} with InterruptStatus {pending_interrupts:#x}",
            line.asserted()
        );
        let console = dev.device();
        assert!(console.output().len() <= OUTPUT_CAP);
        assert!(console.pending_input() <= INPUT_CAP);
        let delivered = console.delivered_input();
        assert!(
            delivered >= last_delivered,
            "delivered input went backwards"
        );
        let pending = u64::try_from(console.pending_input()).expect("at most 64");
        assert!(
            delivered + pending <= accepted,
            "more input than was accepted"
        );
        last_delivered = delivered;
        report.output = u64::try_from(console.output().len()).expect("small") + console.dropped();
        report.delivered = delivered;
    }
    report
}

/// The `vm.toml` parser.
///
/// Input: the config text. Inputs that are not UTF-8 are skipped: `aegis
/// run` reads the file as a string and rejects them before parsing.
///
/// Invariants: a config the parser accepts has memory and vCPU counts in
/// their documented ranges, a command line without NUL, and stdin
/// forwarding only with a virtio-console; on Unix, its kernel path is
/// under the base directory unless the config gave an absolute path.
/// Parsing is deterministic. Nothing panics.
///
/// Returns whether the config was accepted.
///
/// # Panics
///
/// If an invariant breaks.
#[must_use]
pub fn vm_toml(data: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(data) else {
        return false;
    };
    let base = Path::new("/vms/base");
    let parsed = config::parse(text, base);
    assert_eq!(
        parsed,
        config::parse(text, base),
        "parsing is not deterministic"
    );
    let Ok(cfg) = parsed else {
        return false;
    };
    assert!(config::MEMORY_MIB.contains(&cfg.memory_mib), "{cfg:?}");
    assert!(config::VCPUS.contains(&cfg.vcpus), "{cfg:?}");
    assert!(!cfg.cmdline.contains('\0'), "{cfg:?}");
    assert!(!cfg.virtio_console_stdin || cfg.virtio_console, "{cfg:?}");
    // Path::join keeps an absolute path and resolves a relative one. On
    // Windows a drive-relative path such as `C:x` also replaces the base;
    // `aegis run` is Linux-only.
    if cfg!(unix) {
        assert!(
            cfg.kernel.starts_with(base) || cfg.kernel.is_absolute(),
            "{cfg:?}"
        );
    }
    true
}

std::thread_local! {
    /// The input being fuzzed, kept on Windows for the panic hook.
    static CURRENT_INPUT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Run one input for a cargo-fuzz target (`fuzz_targets/`).
///
/// # Panics
///
/// If the harness finds a broken invariant, or `target` is not one of
/// [`TARGETS`].
pub fn fuzz_input(target: &str, data: &[u8]) {
    if cfg!(windows) {
        CURRENT_INPUT.with_borrow_mut(|input| {
            input.clear();
            input.extend_from_slice(data);
        });
    }
    run(target, data).expect("a known target");
}

/// The cargo-fuzz targets' `init`: make crashing inputs reproducible on
/// Windows.
///
/// libfuzzer-sys installs a panic hook that aborts, and libFuzzer then saves
/// the input as `crash-<sha1>`. On Windows a Rust abort is a fast-fail
/// exception that libFuzzer cannot intercept, so nothing is saved. There,
/// this replaces the hook. The new hook prints the panic (with a backtrace
/// if `RUST_BACKTRACE` is set), writes the input to
/// `<artifact prefix>crash-<FNV-1a hash>`, and then aborts. The prefix is
/// libFuzzer's `-artifact_prefix=`, which cargo-fuzz passes, or else the
/// current directory. Elsewhere this does nothing.
pub fn fuzz_init() {
    if !cfg!(windows) {
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{info}\n{}", std::backtrace::Backtrace::capture());
        let prefix = std::env::args()
            .find_map(|a| a.strip_prefix("-artifact_prefix=").map(str::to_owned))
            .unwrap_or_default();
        let saved = CURRENT_INPUT.with_borrow(|input| {
            let path = format!("{prefix}crash-{:016x}", fnv1a(input));
            std::fs::write(&path, input).map(|()| path)
        });
        match saved {
            Ok(path) => eprintln!("aegis-fuzz: crashing input saved to {path}"),
            Err(e) => eprintln!("aegis-fuzz: could not save the crashing input: {e}"),
        }
        std::process::abort();
    }));
}

/// 64-bit FNV-1a, to name saved inputs by their content.
fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3)
    })
}

/// The cargo-fuzz target names, which are also the directory names under
/// `fuzz/regressions/`, `fuzz/corpus/` and `fuzz/artifacts/`.
pub const TARGETS: [&str; 3] = ["virtqueue", "virtio_mmio", "vm_toml"];

/// Run the harness for `target` on `data`.
///
/// # Errors
///
/// If `target` is not one of [`TARGETS`].
///
/// # Panics
///
/// If the harness finds a broken invariant.
pub fn run(target: &str, data: &[u8]) -> Result<(), String> {
    match target {
        "virtqueue" => drop(virtqueue(data)),
        "virtio_mmio" => drop(virtio_mmio(data)),
        "vm_toml" => drop(vm_toml(data)),
        _ => {
            return Err(format!(
                "unknown target {target:?}; expected one of {TARGETS:?}"
            ));
        }
    }
    Ok(())
}

/// Run `target` on `path`: one input file, or every file in a directory
/// (except `*.md` notes). Returns how many inputs ran.
///
/// # Errors
///
/// An unknown target, or a file or directory that cannot be read.
///
/// # Panics
///
/// If an input breaks an invariant: that input reproduces the crash.
pub fn replay(target: &str, path: &Path) -> Result<usize, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    if !path.is_dir() {
        run(target, &read(path)?)?;
        return Ok(1);
    }
    let mut files: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_none_or(|e| e != "md"))
        .collect();
    files.sort();
    for file in &files {
        eprintln!("replaying {}", file.display());
        run(target, &read(file)?)?;
    }
    Ok(files.len())
}

/// Seed inputs that reach past the early checks, for the smoke tests and
/// for a starting corpus (docs/fuzzing.md). The layouts are compact, so a
/// random mutation is likely to land on a ring or descriptor.
pub mod seeds {
    use super::{MmioOp, regs};
    use aegis_core::AccessSize;

    fn put(image: &mut Vec<u8>, at: usize, bytes: &[u8]) {
        if image.len() < at + bytes.len() {
            image.resize(at + bytes.len(), 0);
        }
        image[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// Descriptor `i` of the table at `table`.
    fn desc(
        image: &mut Vec<u8>,
        table: u16,
        i: u16,
        (addr, len, flags, next): (u64, u32, u16, u16),
    ) {
        let at = usize::from(table) + usize::from(i) * 16;
        put(image, at, &addr.to_le_bytes());
        put(image, at + 8, &len.to_le_bytes());
        put(image, at + 12, &flags.to_le_bytes());
        put(image, at + 14, &next.to_le_bytes());
    }

    /// Publish `heads` in the available ring at `avail`.
    fn publish(image: &mut Vec<u8>, avail: u16, heads: &[u16]) {
        for (slot, head) in heads.iter().enumerate() {
            put(
                image,
                usize::from(avail) + 4 + slot * 2,
                &head.to_le_bytes(),
            );
        }
        let count = u16::try_from(heads.len()).expect("a few heads");
        put(image, usize::from(avail) + 2, &count.to_le_bytes());
    }

    /// A queue of size 8 with three published chains, one of two buffers.
    #[must_use]
    pub fn virtqueue() -> Vec<u8> {
        const DESC: u16 = 0x000;
        const AVAIL: u16 = 0x100;
        const USED: u16 = 0x200;
        let mut image = Vec::new();
        desc(&mut image, DESC, 0, (0x400, 0x40, 0, 0));
        desc(&mut image, DESC, 1, (0x500, 0x20, 1, 2)); // NEXT
        desc(&mut image, DESC, 2, (0x600, 0x20, 2, 0)); // WRITE
        desc(&mut image, DESC, 3, (0x700, 0x10, 0, 0));
        publish(&mut image, AVAIL, &[0, 1, 3]);
        let mut data = Vec::new();
        for v in [8u16, DESC, AVAIL, USED] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend_from_slice(&image);
        data
    }

    /// The Linux driver's init sequence for both console queues, a transmit
    /// of three bytes, then host input into a posted receive buffer.
    #[must_use]
    pub fn virtio_mmio() -> Vec<u8> {
        // (descriptor table, available ring, used ring) per queue.
        const RX: (u16, u16, u16) = (0x000, 0x080, 0x100);
        const TX: (u16, u16, u16) = (0x200, 0x280, 0x300);
        const RX_BUFFER: u64 = 0x400;
        const TX_DATA: u16 = 0x480;
        let mut image = Vec::new();
        // Receive queue (0): one 16-byte writable buffer, posted.
        desc(&mut image, RX.0, 0, (RX_BUFFER, 16, 2, 0));
        publish(&mut image, RX.1, &[0]);
        // Transmit queue (1): "hi\n", posted.
        put(&mut image, usize::from(TX_DATA), b"hi\n");
        desc(&mut image, TX.0, 0, (u64::from(TX_DATA), 3, 0, 0));
        publish(&mut image, TX.1, &[0]);

        let w = |offset: u16, value: u32| MmioOp::Write {
            offset,
            size: AccessSize::Dword,
            value,
        };
        let mut ops = vec![
            w(regs::STATUS, 0),
            w(regs::STATUS, 1 | 2),
            w(regs::DRIVER_FEATURES_SEL, 1),
            w(regs::DRIVER_FEATURES, 1), // VIRTIO_F_VERSION_1 (bit 32)
            w(regs::STATUS, 1 | 2 | 8),
        ];
        for (queue, (d, a, u)) in [(0u32, RX), (1, TX)] {
            ops.extend([
                w(regs::QUEUE_SEL, queue),
                w(regs::QUEUE_NUM, 8),
                w(regs::QUEUE_DESC_LOW, u32::from(d)),
                w(regs::QUEUE_DRIVER_LOW, u32::from(a)),
                w(regs::QUEUE_DEVICE_LOW, u32::from(u)),
                w(regs::QUEUE_READY, 1),
            ]);
        }
        ops.extend([
            w(regs::STATUS, 1 | 2 | 8 | 4),
            w(regs::QUEUE_NOTIFY, 1), // transmit; opens the port
            MmioOp::Input {
                bytes: *b"abc\n",
                len: 4,
            },
            MmioOp::Read {
                offset: regs::INTERRUPT_STATUS,
                size: AccessSize::Dword,
            },
            w(regs::INTERRUPT_ACK, 1),
        ]);
        let steps = u16::try_from(ops.len()).unwrap_or(u16::MAX);
        let mut data = steps.to_le_bytes().to_vec();
        for op in ops {
            data.extend_from_slice(&op.encode());
        }
        data.extend_from_slice(&image);
        data
    }

    /// A complete, valid `vm.toml`.
    #[must_use]
    pub fn vm_toml() -> Vec<u8> {
        b"memory_mib = 256\nkernel = \"bzImage\"\ninitrd = \"initramfs.cpio\"\n\
          cmdline = \"console=ttyS0\"\nvcpus = 2\nvirtio_console = true\n\
          virtio_console_stdin = true\n"
            .to_vec()
    }

    /// The seed for each of [`super::TARGETS`], in that order.
    #[must_use]
    pub fn all() -> [(&'static str, Vec<u8>); 3] {
        [
            ("virtqueue", virtqueue()),
            ("virtio_mmio", virtio_mmio()),
            ("vm_toml", vm_toml()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, fixed pseudo-random generator (xorshift64), so the smoke
    /// tests are deterministic and need no dependencies.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn byte(&mut self) -> u8 {
            self.next().to_le_bytes()[0]
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
        }

        fn bytes(&mut self, len: usize) -> Vec<u8> {
            (0..len).map(|_| self.byte()).collect()
        }

        /// `seed` with one to eight bytes overwritten, inserted or removed.
        fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
            let mut out = seed.to_vec();
            for _ in 0..=self.below(8) {
                if out.is_empty() {
                    out.push(0);
                }
                let at = self.below(out.len());
                match self.below(3) {
                    0 => out[at] = self.byte(),
                    1 => out.insert(at, self.byte()),
                    _ => drop(out.remove(at)),
                }
            }
            out
        }

        /// Half mutated seeds, half random bytes of up to `max_len`.
        fn input(&mut self, i: usize, seed: &[u8], max_len: usize) -> Vec<u8> {
            if i.is_multiple_of(2) {
                self.mutate(seed)
            } else {
                let len = self.below(max_len);
                self.bytes(len)
            }
        }
    }

    const RUNS: usize = 4000;

    #[test]
    fn the_seeds_reach_the_interesting_paths() {
        let q = virtqueue(&seeds::virtqueue());
        assert_eq!(
            q,
            QueueReport {
                enabled: true,
                chains: 3
            }
        );
        let m = virtio_mmio(&seeds::virtio_mmio());
        assert!(m.driver_ok, "{m:?}");
        assert_eq!(m.output, 3, "the transmitted \"hi\\n\"");
        assert_eq!(m.delivered, 4, "the input \"abc\\n\"");
        assert!(!m.needs_reset, "{m:?}");
        assert!(vm_toml(&seeds::vm_toml()));
    }

    #[test]
    fn random_and_mutated_inputs_keep_the_virtqueue_invariants() {
        let mut rng = Rng(0x00AE_6150_0001);
        let seed = seeds::virtqueue();
        let (mut enabled, mut chains) = (0, 0);
        for i in 0..RUNS {
            let r = virtqueue(&rng.input(i, &seed, 1024));
            enabled += usize::from(r.enabled);
            chains += r.chains;
        }
        eprintln!("virtqueue: {enabled} of {RUNS} inputs enabled the queue; {chains} chains");
        assert!(
            enabled > RUNS / 10,
            "only {enabled} inputs enabled the queue"
        );
        assert!(chains > RUNS / 10, "only {chains} chains popped");
    }

    #[test]
    fn random_and_mutated_scripts_keep_the_mmio_invariants() {
        let mut rng = Rng(0x00AE_6150_0002);
        let seed = seeds::virtio_mmio();
        let (mut driver_ok, mut delivered, mut resets) = (0, 0, 0);
        for i in 0..RUNS {
            let r = virtio_mmio(&rng.input(i, &seed, 1024));
            driver_ok += usize::from(r.driver_ok);
            delivered += usize::from(r.delivered > 0);
            resets += usize::from(r.needs_reset);
        }
        eprintln!(
            "virtio_mmio: of {RUNS} scripts, {driver_ok} reached DRIVER_OK, \
             {delivered} delivered input, {resets} needed a reset"
        );
        assert!(
            driver_ok > RUNS / 10,
            "only {driver_ok} scripts reached DRIVER_OK"
        );
        assert!(
            delivered > RUNS / 20,
            "only {delivered} scripts delivered input"
        );
        assert!(
            resets > 0,
            "no script drove the device into DEVICE_NEEDS_RESET"
        );
    }

    #[test]
    fn random_and_mutated_configs_keep_the_parser_invariants() {
        let mut rng = Rng(0x00AE_6150_0003);
        let seed = seeds::vm_toml();
        let mut accepted = 0;
        for i in 0..RUNS {
            accepted += usize::from(vm_toml(&rng.input(i, &seed, 256)));
        }
        eprintln!("vm_toml: {accepted} of {RUNS} configs accepted");
        assert!(accepted > 0, "no mutated config was accepted");
    }

    #[test]
    fn mmio_steps_round_trip_through_their_encoding() {
        for op in [
            MmioOp::Write {
                offset: 0x1FF,
                size: AccessSize::Qword,
                value: 0xDEAD_BEEF,
            },
            MmioOp::Read {
                offset: regs::STATUS,
                size: AccessSize::Byte,
            },
            MmioOp::Input {
                bytes: *b"wxyz",
                len: 2,
            },
            MmioOp::Process { queue: 2 },
        ] {
            assert_eq!(MmioOp::decode(op.encode()), op);
        }
    }

    #[test]
    fn saved_inputs_are_named_by_their_fnv1a_hash() {
        // Reference values for 64-bit FNV-1a.
        assert_eq!(fnv1a(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xAF63_DC4C_8601_EC8C);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_F739_67E8);
    }

    #[test]
    fn fuzz_input_runs_the_named_harness() {
        fuzz_input("virtqueue", &seeds::virtqueue());
        if cfg!(windows) {
            CURRENT_INPUT.with_borrow(|input| assert_eq!(*input, seeds::virtqueue()));
        }
    }

    #[test]
    fn replay_runs_one_file_or_every_input_in_a_directory() {
        let dir = std::env::temp_dir().join(format!("aegis-fuzz-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), seeds::virtqueue()).unwrap();
        std::fs::write(dir.join("b"), [0xFF; 9]).unwrap();
        std::fs::write(dir.join("README.md"), "not an input").unwrap();
        assert_eq!(replay("virtqueue", &dir), Ok(2));
        assert_eq!(replay("virtqueue", &dir.join("a")), Ok(1));
        assert!(replay("virtqueue", &dir.join("missing")).is_err());
        assert!(replay("no_such_target", &dir.join("a")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Crash reproduction: every input saved under `fuzz/regressions/<target>/`
    /// (a crash found by the fuzzer, kept once fixed) must still run cleanly.
    #[test]
    fn saved_regression_inputs_still_pass() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("regressions");
        let mut replayed = 0;
        for target in TARGETS {
            let dir = root.join(target);
            if dir.is_dir() {
                replayed += replay(target, &dir).unwrap();
            }
        }
        eprintln!("replayed {replayed} saved regression inputs");
    }

    /// Write the seeds as a starting corpus for cargo-fuzz:
    /// `cargo test --manifest-path fuzz/Cargo.toml -- --ignored write_seed_corpus`.
    #[test]
    #[ignore = "writes fuzz/corpus/<target>/seed; run on purpose before fuzzing"]
    fn write_seed_corpus() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
        for (target, seed) in seeds::all() {
            let dir = root.join(target);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("seed"), seed).unwrap();
        }
    }
}
