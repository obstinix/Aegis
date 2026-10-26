//! Roadmap M3 acceptance tests: boot Linux with `aegis run`, and check the
//! run-loop seccomp filter.
//!
//! The boot test needs `/dev/kvm`, `nasm`, `ld`, and a bzImage. The kernel
//! comes from `AEGIS_TEST_KERNEL`, or else the WSL2 kernel at
//! `/mnt/c/Program Files/WSL/tools/kernel`. The test is `#[ignore]`d, so a
//! plain `cargo test` reports it as ignored. Run it with
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

/// Assemble and link `tests/guests/init_marker.asm` into a static ELF.
fn build_init() -> Vec<u8> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/guests/init_marker.asm");
    let (obj, exe) = (tmp("init_marker.o"), tmp("init_marker"));
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

fn initramfs() -> PathBuf {
    let init = build_init();
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
            name: "init",
            mode: 0o100_755,
            rdev: (0, 0),
            data: &init,
        },
    ]);
    let path = tmp("initramfs.cpio");
    std::fs::write(&path, archive).unwrap();
    path
}

/// Run `aegis` with `args`, killing it after `limit`.
fn aegis(args: &[&str], limit: Duration) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aegis"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
            initramfs().display().to_string()
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
    std::fs::write(&config, "memory_mib = 64\nkernel = \"k\"\nvcpus = 4\n").unwrap();
    let run = aegis(
        &["run", "--config", config.to_str().unwrap()],
        Duration::from_secs(30),
    );
    assert_eq!(run.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&run.stderr).contains("vcpus"));
}
