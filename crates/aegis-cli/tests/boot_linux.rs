//! Roadmap M3, M4h and M6h acceptance tests: boot Linux with `aegis run`
//! (one vCPU; two vCPUs with a virtio-console; then host stdin read by the
//! guest from that console), and check the run-loop seccomp filter.
//!
//! The boot tests need `/dev/kvm`, `nasm`, `ld`, and a bzImage. The kernel
//! comes from `AEGIS_TEST_KERNEL`, or else the WSL2 kernel at
//! `/mnt/c/Program Files/WSL/tools/kernel`. The M4h test also needs a kernel
//! built with `CONFIG_ACPI`, `CONFIG_SMP`, `CONFIG_VIRTIO_MMIO` and
//! `CONFIG_VIRTIO_CONSOLE` (all built in, as in the WSL2 kernel). The tests
//! are `#[ignore]`d, so a plain `cargo test` reports them as ignored. Run
//! them with
//!
//! ```text
//! cargo test -p aegis-cli --test boot_linux -- --include-ignored
//! ```

#![cfg(target_os = "linux")]
// Test helpers: archive offsets and deadlines over small, known sizes.
#![allow(clippy::arithmetic_side_effects)]

use std::io::Read as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const MARKER: &str = "AEGIS_M3_INIT_OK";
const WSL_KERNEL: &str = "/mnt/c/Program Files/WSL/tools/kernel";
const SIGSYS: i32 = 31;

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn kernel() -> PathBuf {
    let path = std::env::var_os("AEGIS_TEST_KERNEL")
        .map_or_else(|| PathBuf::from(WSL_KERNEL), PathBuf::from);
    assert!(
        path.is_file(),
        "no kernel at {} (set AEGIS_TEST_KERNEL to a bzImage)",
        path.display()
    );
    path
}

fn checked(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed: {status}");
}

/// Assemble and link `tests/guests/<name>.asm` into a static ELF.
fn build_init(name: &str) -> Vec<u8> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/guests")
        .join(format!("{name}.asm"));
    let (obj, exe) = (tmp(&format!("{name}.o")), tmp(name));
    checked(
        Command::new("nasm")
            .args(["-f", "elf64", "-w+all", "-o"])
            .arg(&obj)
            .arg(&src),
    );
    checked(
        Command::new("ld")
            .args(["-static", "-nostdlib", "-o"])
            .arg(&exe)
            .arg(&obj),
    );
    std::fs::read(&exe).unwrap()
}

/// One entry of a newc ("070701") cpio archive, the initramfs format.
struct Entry<'a> {
    name: &'a str,
    mode: u32,
    rdev: (u32, u32),
    data: &'a [u8],
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

fn newc(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let trailer = Entry {
        name: "TRAILER!!!",
        mode: 0,
        rdev: (0, 0),
        data: &[],
    };
    for (ino, e) in entries.iter().chain(std::iter::once(&trailer)).enumerate() {
        let nlink = if e.mode & 0o170_000 == 0o040_000 {
            2
        } else {
            1
        };
        let fields = [
            u32::try_from(ino).unwrap().checked_add(1).unwrap(),
            e.mode,
            0, // uid
            0, // gid
            nlink,
            0, // mtime
            u32::try_from(e.data.len()).unwrap(),
            0, // devmajor
            0, // devminor
            e.rdev.0,
            e.rdev.1,
            u32::try_from(e.name.len() + 1).unwrap(),
            0, // check
        ];
        out.extend_from_slice(b"070701");
        for f in fields {
            out.extend_from_slice(format!("{f:08X}").as_bytes());
        }
        out.extend_from_slice(e.name.as_bytes());
        out.push(0);
        pad4(&mut out);
        out.extend_from_slice(e.data);
        pad4(&mut out);
    }
    out
}

/// An initramfs whose `/init` is `tests/guests/<init>.asm`, with the device
/// nodes it uses (`/dev/hvc0` is the virtio-console's port 0) and `/proc`.
fn initramfs(init: &str) -> PathBuf {
    let exe = build_init(init);
    let archive = newc(&[
        Entry {
            name: "dev",
            mode: 0o040_755,
            rdev: (0, 0),
            data: &[],
        },
        Entry {
            name: "dev/console",
            mode: 0o020_600,
            rdev: (5, 1),
            data: &[],
        },
        Entry {
            name: "dev/kmsg",
            mode: 0o020_644,
            rdev: (1, 11),
            data: &[],
        },
        Entry {
            name: "dev/hvc0",
            mode: 0o020_620,
            rdev: (229, 0),
            data: &[],
        },
        Entry {
            name: "proc",
            mode: 0o040_555,
            rdev: (0, 0),
            data: &[],
        },
        Entry {
            name: "init",
            mode: 0o100_755,
            rdev: (0, 0),
            data: &exe,
        },
    ]);
    let path = tmp(&format!("initramfs-{init}.cpio"));
    std::fs::write(&path, archive).unwrap();
    path
}

/// Run `aegis` with `args`, killing it after `limit`.
fn aegis(args: &[&str], limit: Duration) -> Output {
    aegis_with_stdin(args, limit, None)
}

/// Run `aegis` with `args`, killing it after `limit`. With `stdin`, those
/// bytes are written to its stdin, which then stays open (no end of file)
/// until `aegis` exits: it has to stop on its own.
fn aegis_with_stdin(args: &[&str], limit: Duration, stdin: Option<&[u8]>) -> Output {
    use std::io::Write as _;

    let mut command = Command::new(env!("CARGO_BIN_EXE_aegis"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    // Held, not dropped, until the child has exited.
    let _held_open = stdin.map(|bytes| {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(bytes).unwrap();
        pipe.flush().unwrap();
        pipe
    });
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let readers = (
        std::thread::spawn(move || {
            let mut s = Vec::new();
            out.read_to_end(&mut s).map(|_| s)
        }),
        std::thread::spawn(move || {
            let mut s = Vec::new();
            err.read_to_end(&mut s).map(|_| s)
        }),
    );
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("aegis {args:?} did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: readers.0.join().unwrap().unwrap(),
        stderr: readers.1.join().unwrap().unwrap(),
    }
}

#[test]
#[ignore = "requires /dev/kvm, nasm, ld and a bzImage; run with --include-ignored"]
fn boots_linux_to_the_init_marker_under_seccomp() {
    let config = tmp("vm.toml");
    std::fs::write(
        &config,
        format!(
            "memory_mib = 256\nkernel = {:?}\ninitrd = {:?}\n",
            kernel().display().to_string(),
            initramfs("init_marker").display().to_string()
        ),
    )
    .unwrap();

    let started = Instant::now();
    let run = aegis(
        &["run", "--config", config.to_str().unwrap()],
        Duration::from_mins(3),
    );
    let (stdout, stderr) = (
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let report = format!(
        "status {}\n--- stderr\n{stderr}\n--- stdout (last 3000 bytes)\n{}",
        run.status,
        &stdout[stdout.len().saturating_sub(3000)..]
    );

    assert!(
        stderr.contains("seccomp run-loop filter installed"),
        "{report}"
    );
    assert!(
        stdout.contains("Linux version 6."),
        "kernel banner reached the UART console\n{report}"
    );
    assert!(
        stdout.contains(MARKER),
        "/init ran and its marker reached the console\n{report}"
    );
    assert!(
        stderr.contains("guest stopped: Shutdown"),
        "restart by triple fault stops the VM normally\n{report}"
    );
    assert!(run.status.success(), "{report}");
    eprintln!("booted to {MARKER} in {:?}", started.elapsed());
}

/// The interrupt counts, one per CPU, of the `/proc/interrupts` row whose
/// action is a virtio device, or `None` if there is no such row.
fn virtio_interrupts(proc_interrupts: &str) -> Option<Vec<u64>> {
    let row = proc_interrupts.lines().find(|l| {
        l.split_whitespace()
            .last()
            .is_some_and(|a| a.starts_with("virtio"))
    })?;
    Some(
        row.split_whitespace()
            .skip(1)
            .map_while(|t| t.parse().ok())
            .collect(),
    )
}

#[test]
fn virtio_interrupt_row_is_parsed_per_cpu() {
    let table = "           CPU0       CPU1       \n  0:         54          0   IO-APIC   2-edge      timer\n  5:          3         12   IO-APIC   5-level     virtio0\nNMI:          0          0   Non-maskable interrupts\n";
    assert_eq!(virtio_interrupts(table), Some(vec![3, 12]));
    assert_eq!(virtio_interrupts("  0:  1  IO-APIC  2-edge  timer\n"), None);
}

#[test]
#[ignore = "requires /dev/kvm, nasm, ld and a bzImage with ACPI, SMP and virtio-mmio; run with --include-ignored"]
fn boots_two_vcpus_with_a_virtio_console() {
    let config = tmp("vm-m4h.toml");
    std::fs::write(
        &config,
        format!(
            "memory_mib = 256\nkernel = {:?}\ninitrd = {:?}\nvcpus = 2\nvirtio_console = true\n",
            kernel().display().to_string(),
            initramfs("init_m4h").display().to_string()
        ),
    )
    .unwrap();

    let started = Instant::now();
    let run = aegis(
        &["run", "--config", config.to_str().unwrap()],
        Duration::from_mins(3),
    );
    let (stdout, stderr) = (
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let (console, hvc) = stdout
        .split_once("\n--- virtio-console (hvc0) ---\n")
        .unwrap_or((&stdout, ""));
    let report = format!(
        "status {}\n--- stderr\n{stderr}\n--- hvc0\n{hvc}\n--- UART console (last 4000 bytes)\n{}",
        run.status,
        &console[console.len().saturating_sub(4000)..]
    );

    // The kernel found both CPUs through the MADT and started the AP.
    assert!(
        console.contains("Using ACPI (MADT) for SMP configuration information"),
        "{report}"
    );
    assert!(
        console.contains("smp: Brought up 1 node, 2 CPUs"),
        "{report}"
    );
    // ACPICA checks every table's checksum when it loads the tables, and
    // reports a bad one as an "ACPI BIOS Warning" but uses the table anyway,
    // so a wrong checksum shows up only here.
    for complaint in [
        "ACPI BIOS Warning",
        "ACPI BIOS Error",
        "ACPI Warning",
        "ACPI Error",
    ] {
        assert!(!console.contains(complaint), "{complaint}\n{report}");
    }
    assert!(console.contains("AEGIS_M4H_INIT_START"), "{report}");
    assert!(!console.contains("AEGIS_M4H_HVC_FAIL"), "{report}");
    assert!(console.contains("AEGIS_M4H_INIT_DONE"), "{report}");
    // /dev/hvc0 opened, and its writes reached the host through the
    // transmit virtqueue, start to end.
    assert!(hvc.contains("AEGIS_M4H_HVC_OK"), "{report}");
    assert!(hvc.contains("AEGIS_M4H_HVC_END"), "{report}");
    let header: Vec<_> = hvc
        .lines()
        .find(|l| l.trim_start().starts_with("CPU0"))
        .map(|l| l.split_whitespace().collect())
        .unwrap_or_default();
    assert_eq!(header, ["CPU0", "CPU1"], "both CPUs online\n{report}");
    // The used-buffer interrupt (GSI 5, from the DSDT) reached the guest.
    let counts = virtio_interrupts(hvc).unwrap_or_default();
    assert_eq!(counts.len(), 2, "virtio row in /proc/interrupts\n{report}");
    assert!(counts.iter().sum::<u64>() > 0, "{report}");

    // One vCPU triple-faults; the runner kicks the other out of KVM_RUN.
    assert!(stderr.contains("guest stopped: Shutdown"), "{report}");
    assert!(stderr.contains(": Requested after"), "{report}");
    assert!(stderr.contains(" 0 KVM_IRQ_LINE failures"), "{report}");
    assert!(!stderr.contains("needs reset"), "{report}");
    assert!(run.status.success(), "{report}");
    eprintln!(
        "two vCPUs booted with a virtio-console in {:?}; virtio interrupts per CPU {counts:?}",
        started.elapsed()
    );
}

#[test]
#[ignore = "requires /dev/kvm, nasm, ld and a bzImage with ACPI, SMP and virtio-mmio; run with --include-ignored"]
fn guest_reads_host_stdin_from_hvc0() {
    let config = tmp("vm-input.toml");
    std::fs::write(
        &config,
        format!(
            "memory_mib = 256\nkernel = {:?}\ninitrd = {:?}\nvcpus = 2\nvirtio_console = true\nvirtio_console_stdin = true\n",
            kernel().display().to_string(),
            initramfs("init_console_input").display().to_string()
        ),
    )
    .unwrap();
    let input = b"hello from the host\nsecond line\n";

    let started = Instant::now();
    // Stdin stays open after the input, so the worker never sees end of
    // file: the run ends only if the guest's stop also stops the worker.
    let run = aegis_with_stdin(
        &["run", "--config", config.to_str().unwrap()],
        Duration::from_mins(2),
        Some(input),
    );
    let (stdout, stderr) = (
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let (console, hvc) = stdout
        .split_once("\n--- virtio-console (hvc0) ---\n")
        .unwrap_or((&stdout, ""));
    let report = format!(
        "status {}\n--- stderr\n{stderr}\n--- hvc0\n{hvc}\n--- UART console (last 3000 bytes)\n{}",
        run.status,
        &console[console.len().saturating_sub(3000)..]
    );

    assert!(console.contains("AEGIS_INPUT_INIT_START"), "{report}");
    assert!(!console.contains("AEGIS_INPUT_HVC_FAIL"), "{report}");
    assert!(!console.contains("AEGIS_INPUT_READ_FAIL"), "{report}");
    assert!(
        !console.contains("AEGIS_INPUT_TIMEOUT"),
        "no line within 10 s\n{report}"
    );
    assert!(console.contains("AEGIS_INPUT_INIT_DONE"), "{report}");
    // Both lines reached guest userspace, in order, byte for byte.
    let first = hvc.find("AEGIS_RX[hello from the host\n]");
    let second = hvc.find("AEGIS_RX[second line\n]");
    assert!(
        matches!((first, second), (Some(a), Some(b)) if a < b),
        "{report}"
    );
    assert!(hvc.contains("AEGIS_RX_END"), "{report}");

    assert!(stderr.contains("stdin goes to hvc0"), "{report}");
    assert!(
        stderr.contains(&format!("{} bytes in, 0 pending", input.len())),
        "every byte was written into a receive buffer\n{report}"
    );
    assert!(
        stderr.contains(&format!(
            "stdin to hvc0: {} bytes forwarded, ended by the guest stopping",
            input.len()
        )),
        "the worker stopped with the guest, not at end of file\n{report}"
    );
    assert!(stderr.contains("guest stopped: Shutdown"), "{report}");
    assert!(stderr.contains(" 0 KVM_IRQ_LINE failures"), "{report}");
    assert!(!stderr.contains("needs reset"), "{report}");
    assert!(run.status.success(), "{report}");
    eprintln!(
        "guest read {} bytes of host input from hvc0; run took {:?}",
        input.len(),
        started.elapsed()
    );
}

#[test]
fn seccomp_selftest_is_killed_by_sigsys() {
    let run = aegis(&["selftest", "seccomp"], Duration::from_secs(30));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        run.status.signal(),
        Some(SIGSYS),
        "status {}; stderr: {stderr}",
        run.status
    );
    assert!(!stderr.contains("FAILED"), "{stderr}");
}

#[test]
fn run_reports_config_errors_with_status_2() {
    let config = tmp("bad.toml");
    std::fs::write(&config, "memory_mib = 64\nkernel = \"k\"\nballoon = true\n").unwrap();
    let run = aegis(
        &["run", "--config", config.to_str().unwrap()],
        Duration::from_secs(30),
    );
    assert_eq!(run.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&run.stderr).contains("balloon"));
}
