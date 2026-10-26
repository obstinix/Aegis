//! `aegis run`: boot a Linux guest under the KVM backend (roadmap M3), with
//! SMP and a virtio-console (M4h), and `aegis selftest seccomp`.

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
    /// Bytes the guest transmitted on the virtio-console, if one was attached.
    pub(crate) virtio_console: Option<Vec<u8>>,
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

/// Guest-physical window of the virtio-console's virtio-mmio transport.
#[cfg(target_os = "linux")]
const VIRTIO_CONSOLE_MMIO: u64 = 0xD000_0000;
/// I/O APIC input for the virtio-console interrupt (free on this machine
/// model: 0 = PIT, 1 = keyboard, 4 = COM1).
#[cfg(target_os = "linux")]
const VIRTIO_CONSOLE_GSI: u32 = 5;

/// ACPI tables (MADT for SMP, DSDT for virtio) written into the guest, or
/// `None` for a single-vCPU guest without virtio, which keeps the M3 machine.
#[cfg(target_os = "linux")]
fn load_acpi(cfg: &VmConfig, vm: &aegis_kvm::KvmVm) -> Result<Option<u64>, String> {
    use aegis_devices::virtio::mmio::MMIO_SIZE;
    use aegis_kvm::acpi::{self, AcpiConfig, VirtioMmioSpec};

    if cfg.vcpus == 1 && !cfg.virtio_console {
        return Ok(None);
    }
    let virtio_mmio = if cfg.virtio_console {
        vec![VirtioMmioSpec {
            base: u32::try_from(VIRTIO_CONSOLE_MMIO).map_err(|e| e.to_string())?,
            len: u32::try_from(MMIO_SIZE).map_err(|e| e.to_string())?,
            gsi: VIRTIO_CONSOLE_GSI,
        }]
    } else {
        Vec::new()
    };
    let tables = acpi::build(&AcpiConfig {
        vcpus: cfg.vcpus,
        virtio_mmio,
    })
    .map_err(|e| e.to_string())?;
    vm.load_acpi(&tables).map(Some).map_err(|e| e.to_string())
}

/// A VM with `cfg.memory_mib` of RAM at guest-physical 0 and the in-kernel
/// PIC, I/O APIC and PIT.
#[cfg(target_os = "linux")]
fn create_vm(cfg: &VmConfig) -> Result<aegis_kvm::KvmVm, String> {
    use aegis_core::{GuestMemoryMap, MemoryRegion, Permissions};

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
    let mut vm = aegis_kvm::KvmVm::new(map).map_err(|e| e.to_string())?;
    vm.enable_legacy_interrupts().map_err(|e| e.to_string())?;
    Ok(vm)
}

/// vCPU 0 enters the kernel; the others wait for the guest's startup IPIs.
#[cfg(target_os = "linux")]
fn create_vcpus<'vm>(
    vm: &'vm aegis_kvm::KvmVm,
    count: u8,
    entry: &aegis_kvm::LinuxEntry,
) -> Result<Vec<aegis_kvm::KvmVcpu<'vm>>, String> {
    (0..u64::from(count))
        .map(|id| {
            let mut vcpu = vm.create_vcpu(id).map_err(|e| e.to_string())?;
            if id == 0 {
                vcpu.set_linux_entry(vm, entry).map_err(|e| e.to_string())?;
            } else {
                vcpu.set_ap_state(vm).map_err(|e| e.to_string())?;
            }
            Ok(vcpu)
        })
        .collect()
}

/// The stderr summary: the stop reason that ended the guest (the first vCPU
/// whose run was not stopped on request), then one line per vCPU and the
/// virtio-console's state.
#[cfg(target_os = "linux")]
fn summarize(
    normal: bool,
    unassigned: u64,
    uart_dropped: u64,
    results: &[aegis_kvm::smp::VcpuResult],
    virtio: Option<String>,
) -> String {
    use std::fmt::Write as _;

    let decisive = results
        .iter()
        .find(|r| r.outcome.stop != aegis_core::StopReason::Requested)
        .map_or_else(
            || "no vCPU stopped on its own".to_owned(),
            |r| format!("{:?} on vCPU {}", r.outcome.stop, r.id),
        );
    let mut summary = format!(
        "aegis: guest stopped: {decisive} ({}; {unassigned} unassigned port accesses, {uart_dropped} bytes dropped by the UART)",
        if normal { "normal stop" } else { "failure" }
    );
    for r in results {
        let detail = r
            .detail
            .as_deref()
            .map(|d| format!("; {d}"))
            .unwrap_or_default();
        let _ = write!(
            summary,
            "\naegis:   vCPU {}: {:?} after {} exits{detail}",
            r.id, r.outcome.stop, r.outcome.exits
        );
    }
    if let Some(v) = virtio {
        let _ = write!(summary, "\naegis:   virtio-console: {v}");
    }
    summary
}

/// Boot `cfg` and run it until it stops. Every vCPU thread installs the
/// seccomp filter on itself before entering the guest.
#[cfg(target_os = "linux")]
pub(crate) fn run_vm(cfg: &VmConfig) -> Result<RunReport, String> {
    use std::sync::Mutex;

    use aegis_core::{DeviceBus, RunLimits, VmEvent, VmLifecycle, vm_stop_event};
    use aegis_devices::virtio::mmio::MMIO_SIZE;
    use aegis_devices::{Serial16550, VirtioConsole, VirtioMmio};

    let kernel = read_image(&cfg.kernel, "kernel")?;
    let initrd = cfg
        .initrd
        .as_deref()
        .map(|p| read_image(p, "initramfs"))
        .transpose()?;
    let vm = create_vm(cfg)?;
    let rsdp = load_acpi(cfg, &vm)?;
    let entry = vm
        .load_linux(&kernel, initrd.as_deref(), &cfg.cmdline, rsdp)
        .map_err(|e| e.to_string())?;
    drop((kernel, initrd));
    let vcpus = create_vcpus(&vm, cfg.vcpus, &entry)?;

    let mut lifecycle = VmLifecycle::new();
    lifecycle
        .apply(VmEvent::Configure)
        .map_err(|e| e.to_string())?;
    lifecycle.apply(VmEvent::Start).map_err(|e| e.to_string())?;

    let mut uart = Serial16550::default();
    let irq = vm.irq_line(VIRTIO_CONSOLE_GSI);
    let mut virtio = cfg
        .virtio_console
        .then(|| VirtioMmio::new(VirtioConsole::default(), &vm, &irq));

    eprintln!(
        "aegis: seccomp run-loop filter installed on every vCPU thread; booting {} MiB guest with {} vCPU(s){}",
        cfg.memory_mib,
        cfg.vcpus,
        if cfg.virtio_console {
            " and virtio-console"
        } else {
            ""
        }
    );
    let limits = RunLimits {
        max_exits: 50_000_000,
        trace_capacity: 64,
    };
    let (results, unassigned) = {
        let mut bus = DeviceBus::new();
        bus.attach_port(0x3F8, Serial16550::PORTS, &mut uart)
            .map_err(|e| e.to_string())?;
        if let Some(v) = virtio.as_mut() {
            bus.attach_mmio(VIRTIO_CONSOLE_MMIO, MMIO_SIZE, v)
                .map_err(|e| e.to_string())?;
        }
        let bus = Mutex::new(bus);
        let results = aegis_kvm::smp::run(vcpus, &bus, limits, true).map_err(|e| e.to_string())?;
        let unassigned = bus
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unassigned()
            .count;
        (results, unassigned)
    };

    let reasons: Vec<_> = results.iter().map(|r| r.outcome.stop.clone()).collect();
    let event = vm_stop_event(&reasons);
    lifecycle.apply(event).map_err(|e| e.to_string())?;
    let virtio_state = virtio.as_ref().map(|v| {
        format!(
            "status {:#x}, {} bytes, {} dropped, {} KVM_IRQ_LINE failures{}",
            v.status(),
            v.device().output().len(),
            v.device().dropped(),
            irq.failures(),
            v.last_error()
                .map(|e| format!(", needs reset: {e}"))
                .unwrap_or_default()
        )
    });
    Ok(RunReport {
        normal: event == VmEvent::Stop,
        summary: summarize(
            event == VmEvent::Stop,
            unassigned,
            uart.dropped(),
            &results,
            virtio_state,
        ),
        serial: uart.output().to_vec(),
        virtio_console: virtio.as_ref().map(|v| v.device().output().to_vec()),
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
