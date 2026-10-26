//! `aegis run`: boot a Linux guest under the KVM backend (roadmap M3), and
//! `aegis selftest seccomp`.

use std::fmt::Write as _;

use crate::config::VmConfig;

/// Largest kernel or initramfs file accepted.
#[cfg(target_os = "linux")]
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;

/// What a finished run produced.
pub(crate) struct RunReport {
    /// Bytes the guest transmitted on COM1.
    pub(crate) serial: Vec<u8>,
    /// One-line summary for stderr.
    pub(crate) summary: String,
    /// Whether the guest stopped normally (HLT or shutdown).
    pub(crate) normal: bool,
}

/// Escape guest output for a terminal: keep printable ASCII, `\n` and `\t`;
/// drop the `\r` of `\r\n`; show everything else as `\xNN`, so a guest
/// cannot send escape sequences to the host terminal.
pub(crate) fn escape_for_terminal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut iter = bytes.iter().peekable();
    while let Some(&b) = iter.next() {
        match b {
            b'\r' if iter.peek() == Some(&&b'\n') => {}
            b'\n' | b'\t' | 0x20..=0x7E => out.push(char::from(b)),
            // Writing to a String cannot fail.
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn run_vm(_cfg: &VmConfig) -> Result<RunReport, String> {
    Err("`aegis run` needs Linux with KVM; on this workstation, run it inside WSL2".to_owned())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn selftest_seccomp() -> Result<(), String> {
    Err("`aegis selftest seccomp` needs Linux".to_owned())
}

#[cfg(target_os = "linux")]
fn read_image(path: &std::path::Path, what: &str) -> Result<Vec<u8>, String> {
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {what} {shown}: {e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{what} {shown} is larger than {MAX_IMAGE_BYTES} bytes"
        ));
    }
    std::fs::read(path).map_err(|e| format!("cannot read {what} {shown}: {e}"))
}

/// Boot `cfg` and run it until it stops. Seccomp is installed after all
/// setup, immediately before the run loop.
#[cfg(target_os = "linux")]
pub(crate) fn run_vm(cfg: &VmConfig) -> Result<RunReport, String> {
    use aegis_core::{
        GuestMemoryMap, MemoryRegion, Permissions, PortBus, RunLimits, VmEvent, VmLifecycle,
        run_vcpu,
    };
    use aegis_devices::Serial16550;
    use aegis_kvm::KvmVm;

    let kernel = read_image(&cfg.kernel, "kernel")?;
    let initrd = cfg
        .initrd
        .as_deref()
        .map(|p| read_image(p, "initramfs"))
        .transpose()?;
    let ram = cfg
        .memory_mib
        .checked_mul(1024 * 1024)
        .ok_or("memory size overflows")?;

    let map = GuestMemoryMap::new(
        &[MemoryRegion {
            start: 0,
            size: ram,
            permissions: Permissions::RWX,
        }],
        ram,
    )
    .map_err(|e| e.to_string())?;
    let mut vm = KvmVm::new(map).map_err(|e| e.to_string())?;
    vm.enable_legacy_interrupts().map_err(|e| e.to_string())?;
    let entry = vm
        .load_linux(&kernel, initrd.as_deref(), &cfg.cmdline)
        .map_err(|e| e.to_string())?;
    drop((kernel, initrd));
    let mut vcpu = vm.create_vcpu(0).map_err(|e| e.to_string())?;
    vcpu.set_linux_entry(&vm, &entry)
        .map_err(|e| e.to_string())?;

    let mut lifecycle = VmLifecycle::new();
    lifecycle
        .apply(VmEvent::Configure)
        .map_err(|e| e.to_string())?;
    lifecycle.apply(VmEvent::Start).map_err(|e| e.to_string())?;
    let mut uart = Serial16550::default();

    eprintln!(
        "aegis: seccomp run-loop filter installed; booting {} MiB guest",
        cfg.memory_mib
    );
    aegis_kvm::seccomp::apply_run_loop_filter().map_err(|e| e.to_string())?;

    let limits = RunLimits {
        max_exits: 50_000_000,
        trace_capacity: 64,
    };
    let (outcome, unassigned) = {
        let mut bus = PortBus::new();
        bus.attach(0x3F8, Serial16550::PORTS, &mut uart)
            .map_err(|e| e.to_string())?;
        let outcome =
            run_vcpu(&mut vcpu, &mut bus, &mut lifecycle, limits).map_err(|e| e.to_string())?;
        (outcome, bus.unassigned().count)
    };
    let detail = vcpu
        .last_unhandled_detail()
        .map(|d| format!("; {d}"))
        .unwrap_or_default();
    Ok(RunReport {
        normal: outcome.stop.is_normal(),
        summary: format!(
            "aegis: guest stopped: {:?} after {} exits ({unassigned} unassigned port accesses, {} bytes dropped by the UART){detail}",
            outcome.stop,
            outcome.exits,
            uart.dropped()
        ),
        serial: uart.output().to_vec(),
    })
}

/// Install the run-loop seccomp filter, then attempt a forbidden syscall
/// (`openat`). Success means the process is killed by `SIGSYS` and this
/// never returns.
#[cfg(target_os = "linux")]
pub(crate) fn selftest_seccomp() -> Result<(), String> {
    eprintln!("aegis: installing the run-loop filter, then calling openat; expect SIGSYS");
    aegis_kvm::seccomp::apply_run_loop_filter().map_err(|e| e.to_string())?;
    let opened = std::fs::File::open("/proc/self/status");
    Err(format!(
        "seccomp selftest FAILED: openat was not blocked ({})",
        opened.map_or_else(|e| e.to_string(), |_| "file opened".to_owned())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_control_bytes_but_keeps_text() {
        assert_eq!(
            escape_for_terminal(b"Linux version 6.18\r\nok\tdone\n"),
            "Linux version 6.18\nok\tdone\n"
        );
        assert_eq!(escape_for_terminal(b"\x1b[2Jboom\x07"), "\\x1b[2Jboom\\x07");
        assert_eq!(escape_for_terminal(b"lone\rcr"), "lone\\x0dcr");
        assert_eq!(escape_for_terminal(&[0xFF, 0x00]), "\\xff\\x00");
    }
}
