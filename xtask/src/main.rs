//! Aegis build and test orchestration (ADR-0003, ADR-0010).
//!
//! `cargo xtask uefi-test [--timeout <seconds>] [--release]` is the nested
//! bare-metal harness (roadmap B0):
//! 1. checks for Linux, an openable `/dev/kvm`, `qemu-system-x86_64` and
//!    OVMF (`AEGIS_OVMF`, or the distribution's `OVMF.fd`);
//! 2. builds `aegis-uefi` for `x86_64-unknown-uefi`;
//! 3. writes a FAT16 image with it as `\EFI\BOOT\BOOTX64.EFI` (see [`fat`]);
//! 4. boots the image with `qemu-system-x86_64 -enable-kvm -cpu host -bios
//!    OVMF.fd`, nested inside WSL2 on this workstation, with the serial port
//!    captured to a file;
//! 5. passes only if the serial log contains the `AEGIS_B0_HELLO` marker and
//!    QEMU exited by itself (the application powers the VM off) within the
//!    timeout (default 60 s).
//!
//! A failure prints the reason, the QEMU command, the serial log (control
//! bytes escaped) and QEMU's stderr. A missing prerequisite is reported as a
//! skip, never as a pass.
//!
//! `cargo xtask repro-test` checks that the UEFI image builds reproducibly
//! (security analysis §4): `aegis-uefi.efi` is built twice, in debug and in
//! release, each time into a freshly emptied target directory. Both builds,
//! and the FAT images made from them, must be byte-identical. It needs no
//! KVM, so it runs anywhere, including CI.
//!
//! Exit status: 0 passed, 1 failed, 2 usage error, 77 skipped.

mod fat;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// What `aegis-uefi` prints when it runs.
const MARKER: &str = "AEGIS_B0_HELLO";
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(1);
const UEFI_TARGET: &str = "x86_64-unknown-uefi";
/// Where distributions install a combined (code + variables) OVMF image.
const OVMF_CANDIDATES: &[&str] = &[
    "/usr/share/ovmf/OVMF.fd",
    "/usr/share/OVMF/OVMF.fd",
    "/usr/share/qemu/OVMF.fd",
];
/// How much of the serial log a failure report shows.
const LOG_TAIL: usize = 16 * 1024;
const EXIT_SKIPPED: u8 = 77;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  uefi-test [--timeout <seconds>] [--release]
      build aegis-uefi, boot it under QEMU/KVM with OVMF, and check its
      serial output (Linux with KVM; on this workstation, WSL2)
  repro-test
      build aegis-uefi twice from clean target directories (debug and
      release) and check that the binaries and FAT images are identical
  help";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Task {
    UefiTest { timeout: Duration, release: bool },
    ReproTest,
    Help,
}

fn parse_args(args: &[String]) -> Result<Task, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.split_first() {
        None | Some((&("help" | "--help" | "-h"), [])) => Ok(Task::Help),
        Some((&"uefi-test", rest)) => {
            let (mut timeout, mut release) = (DEFAULT_TIMEOUT, false);
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match *arg {
                    "--release" => release = true,
                    "--timeout" => {
                        let secs = rest
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .filter(|s| (1..=3600).contains(s))
                            .ok_or("--timeout needs a number of seconds, 1..=3600")?;
                        timeout = Duration::from_secs(secs);
                    }
                    other => return Err(format!("uefi-test: unknown argument `{other}`")),
                }
            }
            Ok(Task::UefiTest { timeout, release })
        }
        Some((&"repro-test", [])) => Ok(Task::ReproTest),
        Some((&"repro-test", [other, ..])) => {
            Err(format!("repro-test: unknown argument `{other}`"))
        }
        Some((other, _)) => Err(format!("unknown command `{other}`")),
    }
}

/// Why the run passed or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Passed,
    /// QEMU exited normally but the marker never appeared.
    NoMarker,
    /// QEMU was still running at the deadline and was killed.
    TimedOut {
        marker: bool,
    },
    /// QEMU itself failed (bad arguments, firmware or KVM trouble).
    QemuFailed(Option<i32>),
}

/// The verdict for a run whose serial log is `serial`, given QEMU's exit
/// code (`None` if it was killed by a signal) and whether it hit the
/// deadline. A pass needs both the marker and a clean exit: the app powers
/// the VM off, so a VM still running at the deadline is a failure even if
/// the marker appeared.
fn judge(serial: &[u8], exit: Option<i32>, timed_out: bool) -> Verdict {
    let marker = serial.windows(MARKER.len()).any(|w| w == MARKER.as_bytes());
    match (timed_out, exit) {
        (true, _) => Verdict::TimedOut { marker },
        (false, Some(0)) if marker => Verdict::Passed,
        (false, Some(0)) => Verdict::NoMarker,
        (false, code) => Verdict::QemuFailed(code),
    }
}

/// The QEMU command line: q35 with KVM and the host CPU model (so B1 sees
/// VMX), one vCPU, no default devices, the image read-only on virtio-blk,
/// the first serial port captured to `serial`, and exit rather than reboot.
fn qemu_args(firmware: &Path, image: &Path, serial: &Path) -> Result<Vec<OsString>, String> {
    for path in [firmware, image, serial] {
        // QEMU option syntax would split a path at a comma.
        if path.to_string_lossy().contains(',') {
            return Err(format!(
                "{} contains a comma, which QEMU option syntax cannot carry",
                path.display()
            ));
        }
    }
    let mut drive = OsString::from("if=virtio,format=raw,readonly=on,file=");
    drive.push(image);
    let mut serial_spec = OsString::from("file:");
    serial_spec.push(serial);
    let mut args: Vec<OsString> = [
        "-nodefaults",
        "-machine",
        "q35",
        "-enable-kvm",
        "-cpu",
        "host",
        "-smp",
        "1",
        "-m",
        "256M",
        "-display",
        "none",
        "-no-reboot",
        "-bios",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(firmware.into());
    args.push("-drive".into());
    args.push(drive);
    args.push("-serial".into());
    args.push(serial_spec);
    Ok(args)
}

/// The first existing firmware file: `override_path` if given (it must
/// exist), else the first of `candidates`.
fn find_firmware(override_path: Option<PathBuf>, candidates: &[&str]) -> Result<PathBuf, String> {
    if let Some(path) = override_path {
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("AEGIS_OVMF={} is not a file", path.display()))
        };
    }
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "no OVMF firmware at {} (install `ovmf`, or set AEGIS_OVMF)",
                candidates.join(", ")
            )
        })
}

/// Escape a log for a terminal: printable ASCII, `\n` and `\t` stay; the `\r`
/// of `\r\n` is dropped; everything else (the firmware's ANSI sequences,
/// say) shows as `\xNN`.
fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut iter = bytes.iter().peekable();
    while let Some(&b) = iter.next() {
        match b {
            b'\r' if iter.peek() == Some(&&b'\n') => {}
            b'\n' | b'\t' | 0x20..=0x7E => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out
}

/// The path of the `aegis-uefi` executable in `cargo build
/// --message-format=json` output.
fn efi_artifact(cargo_json: &str) -> Option<PathBuf> {
    cargo_json
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|msg| msg["reason"] == "compiler-artifact" && msg["target"]["name"] == "aegis-uefi")
        .find_map(|msg| msg["executable"].as_str().map(PathBuf::from))
}

/// The workspace's target directory, from `cargo metadata` (so it follows
/// `CARGO_TARGET_DIR`).
fn target_dir(cargo: &OsString) -> Result<PathBuf, String> {
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed: {}", out.status));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    meta["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata has no target_directory".to_owned())
}

/// Build `aegis-uefi` and return the `.efi` path. `target_dir` overrides
/// the workspace's target directory.
fn build_efi(
    cargo: &OsString,
    release: bool,
    target_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let mut cmd = Command::new(cargo);
    cmd.args([
        "build",
        "--package",
        "aegis-uefi",
        "--target",
        UEFI_TARGET,
        "--message-format=json-render-diagnostics",
    ]);
    if release {
        cmd.arg("--release");
    }
    if let Some(dir) = target_dir {
        cmd.arg("--target-dir").arg(dir);
    }
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!("building aegis-uefi failed: {}", out.status));
    }
    efi_artifact(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "cargo did not report an aegis-uefi executable".to_owned())
}

/// Why the test cannot run here, if it cannot.
fn skip_reason() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return Some(
            "uefi-test needs Linux with KVM (on this workstation, run it in WSL2)".to_owned(),
        );
    }
    if let Err(e) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
    {
        return Some(format!("cannot open /dev/kvm: {e}"));
    }
    match Command::new("qemu-system-x86_64").arg("--version").output() {
        Ok(out) if out.status.success() => None,
        Ok(out) => Some(format!(
            "qemu-system-x86_64 --version failed: {}",
            out.status
        )),
        Err(e) => Some(format!("cannot run qemu-system-x86_64: {e}")),
    }
}

/// What a QEMU run produced.
struct QemuRun {
    exit: Option<i32>,
    timed_out: bool,
    stderr: Vec<u8>,
    elapsed: Duration,
}

fn run_qemu(args: &[OsString], timeout: Duration) -> Result<QemuRun, String> {
    let started = Instant::now();
    let mut child = Command::new("qemu-system-x86_64")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start qemu-system-x86_64: {e}"))?;
    let mut err = child.stderr.take().ok_or("no stderr pipe")?;
    let reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.read_to_end(&mut s);
        s
    });
    let deadline = started.checked_add(timeout).unwrap_or(started);
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break (status, false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break (child.wait().map_err(|e| e.to_string())?, true);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok(QemuRun {
        exit: status.code(),
        timed_out,
        stderr: reader.join().unwrap_or_default(),
        elapsed: started.elapsed(),
    })
}

fn report_failure(why: &str, args: &[OsString], serial: &[u8], stderr: &[u8]) {
    let shown: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    eprintln!("xtask: uefi-test FAILED: {why}");
    eprintln!("xtask: command: qemu-system-x86_64 {}", shown.join(" "));
    let tail = &serial[serial.len().saturating_sub(LOG_TAIL)..];
    eprintln!(
        "--- serial log ({} bytes{}) ---\n{}",
        serial.len(),
        if tail.len() < serial.len() {
            ", last part"
        } else {
            ""
        },
        escape(tail)
    );
    eprintln!("--- qemu stderr ---\n{}", escape(stderr));
}

fn uefi_test(timeout: Duration, release: bool) -> Result<ExitCode, String> {
    if let Some(reason) = skip_reason() {
        eprintln!("xtask: uefi-test SKIPPED: {reason}");
        return Ok(ExitCode::from(EXIT_SKIPPED));
    }
    let firmware = match find_firmware(
        std::env::var_os("AEGIS_OVMF").map(PathBuf::from),
        OVMF_CANDIDATES,
    ) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("xtask: uefi-test SKIPPED: {reason}");
            return Ok(ExitCode::from(EXIT_SKIPPED));
        }
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    eprintln!("xtask: building aegis-uefi for {UEFI_TARGET}");
    let efi_path = build_efi(&cargo, release, None)?;
    let efi =
        std::fs::read(&efi_path).map_err(|e| format!("cannot read {}: {e}", efi_path.display()))?;
    let dir = target_dir(&cargo)?.join("uefi-test");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let image = dir.join("esp.img");
    let serial = dir.join("serial.log");
    let img = fat::boot_image(&efi).map_err(|e| e.to_string())?;
    std::fs::write(&image, img).map_err(|e| format!("cannot write {}: {e}", image.display()))?;
    // A stale log must not satisfy this run.
    match std::fs::remove_file(&serial) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot remove the old {}: {e}", serial.display())),
    }
    eprintln!(
        "xtask: wrote {} (FAT16, {} bytes; BOOTX64.EFI {} bytes from {})",
        image.display(),
        fat::IMAGE_BYTES,
        efi.len(),
        efi_path.display()
    );

    let args = qemu_args(&firmware, &image, &serial)?;
    eprintln!(
        "xtask: booting under QEMU/KVM with {} (timeout {} s)",
        firmware.display(),
        timeout.as_secs()
    );
    let run = run_qemu(&args, timeout)?;
    let log = std::fs::read(&serial).unwrap_or_default();
    match judge(&log, run.exit, run.timed_out) {
        Verdict::Passed => {
            let line = String::from_utf8_lossy(&log)
                .lines()
                .find(|l| l.contains(MARKER))
                .map(|l| escape(l.as_bytes()))
                .unwrap_or_default();
            eprintln!(
                "xtask: uefi-test PASSED in {:.1} s: the VM powered off after printing\n  {}",
                run.elapsed.as_secs_f64(),
                line.trim()
            );
            Ok(ExitCode::SUCCESS)
        }
        verdict => {
            let why = match verdict {
                Verdict::NoMarker => {
                    format!("QEMU exited normally, but `{MARKER}` is not in the serial log")
                }
                Verdict::TimedOut { marker: true } => format!(
                    "`{MARKER}` appeared, but the VM did not power off within {} s",
                    timeout.as_secs()
                ),
                Verdict::TimedOut { marker: false } => {
                    format!(
                        "no `{MARKER}` and no power-off within {} s",
                        timeout.as_secs()
                    )
                }
                Verdict::QemuFailed(code) => format!("QEMU failed (exit {code:?})"),
                Verdict::Passed => unreachable!(),
            };
            report_failure(&why, &args, &log, &run.stderr);
            Ok(ExitCode::FAILURE)
        }
    }
}

/// The offset of the first byte where `a` and `b` differ (the shorter
/// length if one is a prefix of the other), or `None` if they are equal.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Compare two builds of the same artifact; `Err` names the first difference.
fn compare(what: &str, a: &[u8], b: &[u8]) -> Result<(), String> {
    match first_difference(a, b) {
        None => Ok(()),
        Some(offset) => Err(format!(
            "{what} differs between two clean builds: first difference at byte {offset:#x} \
             (sizes {} and {})",
            a.len(),
            b.len()
        )),
    }
}

/// Build `aegis-uefi` into an emptied `dir` and return the `.efi` bytes.
fn clean_build(cargo: &OsString, release: bool, dir: &Path) -> Result<Vec<u8>, String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot empty {}: {e}", dir.display())),
    }
    let efi = build_efi(cargo, release, Some(dir))?;
    std::fs::read(&efi).map_err(|e| format!("cannot read {}: {e}", efi.display()))
}

fn repro_test() -> Result<ExitCode, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let base = target_dir(&cargo)?.join("repro-test");
    let mut failures = Vec::new();
    for (profile, release) in [("debug", false), ("release", true)] {
        eprintln!("xtask: building aegis-uefi ({profile}) twice from clean target directories");
        let first = clean_build(&cargo, release, &base.join(profile).join("first"))?;
        let second = clean_build(&cargo, release, &base.join(profile).join("second"))?;
        let images = (
            fat::boot_image(&first).map_err(|e| e.to_string())?,
            fat::boot_image(&second).map_err(|e| e.to_string())?,
        );
        let result = compare(&format!("aegis-uefi.efi ({profile})"), &first, &second)
            .and_then(|()| compare(&format!("esp.img ({profile})"), &images.0, &images.1));
        match result {
            Ok(()) => eprintln!(
                "xtask: {profile}: aegis-uefi.efi ({} bytes) and esp.img are identical",
                first.len()
            ),
            Err(e) => failures.push(e),
        }
    }
    // The builds are only needed for the comparison.
    let _ = std::fs::remove_dir_all(&base);
    if failures.is_empty() {
        eprintln!("xtask: repro-test PASSED");
        Ok(ExitCode::SUCCESS)
    } else {
        for f in &failures {
            eprintln!("xtask: repro-test FAILED: {f}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Task::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Task::UefiTest { timeout, release }) => {
            uefi_test(timeout, release).unwrap_or_else(|e| {
                eprintln!("xtask: uefi-test FAILED: {e}");
                ExitCode::FAILURE
            })
        }
        Ok(Task::ReproTest) => repro_test().unwrap_or_else(|e| {
            eprintln!("xtask: repro-test FAILED: {e}");
            ExitCode::FAILURE
        }),
        Err(e) => {
            eprintln!("xtask: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_commands_and_options() {
        assert_eq!(parse_args(&args(&[])), Ok(Task::Help));
        assert_eq!(parse_args(&args(&["help"])), Ok(Task::Help));
        assert_eq!(parse_args(&args(&["repro-test"])), Ok(Task::ReproTest));
        assert!(parse_args(&args(&["repro-test", "--release"])).is_err());
        assert_eq!(
            parse_args(&args(&["uefi-test"])),
            Ok(Task::UefiTest {
                timeout: DEFAULT_TIMEOUT,
                release: false
            })
        );
        assert_eq!(
            parse_args(&args(&["uefi-test", "--release", "--timeout", "30"])),
            Ok(Task::UefiTest {
                timeout: Duration::from_secs(30),
                release: true
            })
        );
        for bad in [
            &["uefi-test", "--timeout"][..],
            &["uefi-test", "--timeout", "0"],
            &["uefi-test", "--timeout", "3601"],
            &["uefi-test", "--timeout", "soon"],
            &["uefi-test", "--fast"],
            &["boot"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_pass_needs_the_marker_and_a_clean_power_off() {
        let log = b"\x1b[2J BdsDxe: starting Boot0001\r\nAEGIS_B0_HELLO: aegis-uefi 0.0.0\r\n";
        assert_eq!(judge(log, Some(0), false), Verdict::Passed);
        assert_eq!(
            judge(b"BdsDxe: no bootable option\r\n", Some(0), false),
            Verdict::NoMarker
        );
        assert_eq!(judge(b"", Some(0), false), Verdict::NoMarker);
        assert_eq!(
            judge(log, None, true),
            Verdict::TimedOut { marker: true },
            "printed but never powered off"
        );
        assert_eq!(judge(b"", None, true), Verdict::TimedOut { marker: false });
        assert_eq!(judge(log, Some(1), false), Verdict::QemuFailed(Some(1)));
        assert_eq!(
            judge(log, None, false),
            Verdict::QemuFailed(None),
            "killed by a signal"
        );
        assert_eq!(
            judge(b"AEGIS_B0_HELL", Some(0), false),
            Verdict::NoMarker,
            "partial marker"
        );
    }

    #[test]
    fn qemu_runs_nested_kvm_with_ovmf_and_captures_serial() {
        let a = qemu_args(
            Path::new("/fw/OVMF.fd"),
            Path::new("/t/uefi-test/esp.img"),
            Path::new("/t/uefi-test/serial.log"),
        )
        .unwrap();
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let pair = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
        assert!(a.contains(&"-enable-kvm".to_owned()));
        assert!(a.contains(&"-nodefaults".to_owned()));
        assert!(
            a.contains(&"-no-reboot".to_owned()),
            "a reboot ends the run"
        );
        assert_eq!(pair("-cpu").as_deref(), Some("host"));
        assert_eq!(pair("-bios").as_deref(), Some("/fw/OVMF.fd"));
        assert_eq!(
            pair("-drive").as_deref(),
            Some("if=virtio,format=raw,readonly=on,file=/t/uefi-test/esp.img")
        );
        assert_eq!(
            pair("-serial").as_deref(),
            Some("file:/t/uefi-test/serial.log")
        );
        assert!(
            qemu_args(
                Path::new("/fw/OVMF.fd"),
                Path::new("/a,b/esp.img"),
                Path::new("/s")
            )
            .is_err(),
            "a comma would split the -drive option"
        );
    }

    #[test]
    fn firmware_override_must_exist_and_candidates_are_tried_in_order() {
        let dir = std::env::temp_dir().join(format!("aegis-xtask-fw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let second = dir.join("second.fd");
        std::fs::write(&second, b"fw").unwrap();
        let missing = dir.join("missing.fd");
        let candidates = [missing.to_str().unwrap(), second.to_str().unwrap()];
        assert_eq!(find_firmware(None, &candidates), Ok(second.clone()));
        assert_eq!(find_firmware(Some(second.clone()), &[]), Ok(second.clone()));
        assert!(find_firmware(Some(missing.clone()), &candidates).is_err());
        let err = find_firmware(None, &[missing.to_str().unwrap()]).unwrap_err();
        assert!(err.contains("AEGIS_OVMF"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn build_comparison_finds_the_first_difference() {
        assert_eq!(first_difference(b"same", b"same"), None);
        assert_eq!(first_difference(b"", b""), None);
        assert_eq!(first_difference(b"abcd", b"abXd"), Some(2));
        assert_eq!(first_difference(b"abc", b"abcd"), Some(3), "a prefix");
        assert_eq!(first_difference(b"abcd", b"ab"), Some(2));
        assert!(compare("x", b"one", b"one").is_ok());
        let err = compare("aegis-uefi.efi (debug)", b"one", b"onf").unwrap_err();
        assert!(
            err.contains("byte 0x2") && err.contains("aegis-uefi.efi (debug)"),
            "{err}"
        );
    }

    #[test]
    fn logs_are_escaped_for_the_terminal() {
        assert_eq!(escape(b"ok\r\nnext\tline\n"), "ok\nnext\tline\n");
        assert_eq!(escape(b"\x1b[2Jclear"), "\\x1b[2Jclear");
        assert_eq!(escape(b"lone\rcr\xff"), "lone\\x0dcr\\xff");
    }

    #[test]
    fn the_efi_artifact_is_found_in_cargo_json() {
        let json = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"uefi_raw"},"executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"aegis-uefi"},"executable":"/t/x86_64-unknown-uefi/debug/aegis-uefi.efi"}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\nnot json\n"
        );
        assert_eq!(
            efi_artifact(json),
            Some(PathBuf::from("/t/x86_64-unknown-uefi/debug/aegis-uefi.efi"))
        );
        assert_eq!(efi_artifact(r#"{"reason":"build-finished"}"#), None);
    }
}
