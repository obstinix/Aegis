//! `aegis run`: boot a Linux guest under the KVM backend (roadmap M3), with
//! SMP and a virtio-console (M4h), and `aegis selftest seccomp`.
//!
//! Guest console output is streamed to stdout while the guest runs (see
//! [`crate::stream`]): the UART as it is, hvc0 lines tagged `hvc0| `.

use crate::config::VmConfig;

/// Largest kernel or initramfs file accepted.
#[cfg(target_os = "linux")]
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;

/// What a finished run produced. Its console output has already been
/// streamed to stdout.
pub(crate) struct RunReport {
    /// Summary for stderr.
    pub(crate) summary: String,
    /// Whether the guest stopped normally (HLT or shutdown).
    pub(crate) normal: bool,
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

/// The virtio-console transport as `aegis run` builds it, streaming its
/// output.
#[cfg(target_os = "linux")]
type Console<'a> = aegis_devices::VirtioMmio<
    aegis_devices::VirtioConsole<crate::stream::Sink<'a>>,
    &'a aegis_kvm::KvmVm,
    &'a aegis_kvm::KvmIrqLine<'a>,
>;

/// The UART as `aegis run` builds it, streaming its output.
#[cfg(target_os = "linux")]
type Uart<'a> = aegis_devices::Serial16550<crate::stream::Sink<'a>>;

/// The output writer: sandbox this thread with the run-loop filter (it only
/// writes to stdout), then write the guest's console output as it arrives
/// until the stream is closed. A writer that cannot sandbox itself shows
/// nothing, so no thread runs unfiltered, but it keeps draining, so the
/// devices never wait.
#[cfg(target_os = "linux")]
fn write_output(stream: &crate::stream::OutputStream) -> crate::stream::StreamStats {
    if let Err(e) = aegis_kvm::seccomp::apply_run_loop_filter() {
        let mut stats = stream.drain_into(&mut std::io::sink());
        let kept = stats
            .received
            .iter()
            .sum::<u64>()
            .saturating_sub(stats.dropped.iter().sum());
        stats.unwritten = kept;
        stats.write_error = Some(format!("the output thread could not be sandboxed: {e}"));
        return stats;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    stream.drain_into(&mut out)
}

/// Ends the host threads if the run unwinds (a vCPU thread panicked).
/// Otherwise the scope would wait forever for the stdin worker, which waits
/// for the stop flag, and the output writer, which waits for the close.
#[cfg(target_os = "linux")]
struct EndOnUnwind<'a> {
    stop: &'a std::sync::atomic::AtomicBool,
    stream: &'a crate::stream::OutputStream,
}

#[cfg(target_os = "linux")]
impl Drop for EndOnUnwind<'_> {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.stream.close();
    }
}

/// The stdin worker: register with the SMP runner (so it can be kicked out
/// of a blocked read), sandbox this thread, then forward stdin to the
/// console until end of file or `stop`. A worker that cannot sandbox itself
/// reads nothing, and the guest runs without input.
#[cfg(target_os = "linux")]
fn forward_stdin(
    console: &std::sync::Mutex<Console<'_>>,
    stop: &std::sync::atomic::AtomicBool,
    host: &aegis_kvm::smp::HostThread,
) -> aegis_kvm::input::InputReport {
    use aegis_kvm::input::{InputReport, STDIN, forward};

    let _registered = host.enter();
    if let Err(e) = aegis_kvm::seccomp::apply_input_worker_filter() {
        return InputReport::failed(e.to_string());
    }
    forward(STDIN, stop, &mut |bytes| {
        console
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .input(bytes)
    })
}

/// What [`run_guest`] produced.
#[cfg(target_os = "linux")]
struct GuestRun {
    results: Vec<aegis_kvm::smp::VcpuResult>,
    unassigned: u64,
    input: Option<aegis_kvm::input::InputReport>,
    output: crate::stream::StreamStats,
}

/// Run `vcpus` over the UART and the optional virtio-console until the
/// guest stops, streaming their output through `stream`, with the stdin
/// worker if `stdin` is set.
///
/// The output writer and the stdin worker start before the vCPUs, because
/// the coordinator may not create threads once it is sandboxed. The worker
/// sees the same stop flag as the vCPUs, and the runner kicks and waits for
/// it like a vCPU. Once the vCPUs and the worker are done, the stream is
/// closed and the writer finishes what is queued, so all output is written
/// before the summary. Both threads are ended on every path, including a
/// runner that fails to start or a vCPU thread that panics.
#[cfg(target_os = "linux")]
fn run_guest(
    vcpus: Vec<aegis_kvm::KvmVcpu<'_>>,
    uart: &mut Uart<'_>,
    console: Option<&std::sync::Mutex<Console<'_>>>,
    stdin: bool,
    stream: &crate::stream::OutputStream,
) -> Result<GuestRun, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, PoisonError};

    use aegis_core::{DeviceBus, RunLimits};
    use aegis_devices::Serial16550;
    use aegis_devices::virtio::mmio::MMIO_SIZE;
    use aegis_kvm::input::InputReport;
    use aegis_kvm::smp::{HostThread, SharedMmio};

    use crate::stream::StreamStats;

    let limits = RunLimits {
        max_exits: 50_000_000,
        trace_capacity: 64,
    };
    let mut shared = console.map(SharedMmio);
    let mut bus = DeviceBus::new();
    bus.attach_port(0x3F8, Serial16550::PORTS, uart)
        .map_err(|e| e.to_string())?;
    if let Some(device) = shared.as_mut() {
        bus.attach_mmio(VIRTIO_CONSOLE_MMIO, MMIO_SIZE, device)
            .map_err(|e| e.to_string())?;
    }
    let bus = Mutex::new(bus);
    let stop = AtomicBool::new(false);
    let host = HostThread::new();
    let (results, input, output) = std::thread::scope(|s| {
        let _end = EndOnUnwind {
            stop: &stop,
            stream,
        };
        let writer = s.spawn(|| write_output(stream));
        let worker = console
            .filter(|_| stdin)
            .map(|c| s.spawn(|| forward_stdin(c, &stop, &host)));
        let host_threads: &[&HostThread] = if worker.is_some() { &[&host] } else { &[] };
        let results = aegis_kvm::smp::run(vcpus, &bus, limits, true, &stop, host_threads);
        stop.store(true, Ordering::Release);
        let input = worker.map(|w| {
            w.join()
                .unwrap_or_else(|_| InputReport::failed("the stdin worker panicked".to_owned()))
        });
        // No device produces output any more: let the writer finish.
        stream.close();
        let output = writer.join().unwrap_or_else(|_| StreamStats {
            write_error: Some("the output writer panicked".to_owned()),
            ..StreamStats::default()
        });
        (results, input, output)
    });
    let results = results.map_err(|e| e.to_string())?;
    let unassigned = bus
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .unassigned()
        .count;
    Ok(GuestRun {
        results,
        unassigned,
        input,
        output,
    })
}

/// One line on the streamed output.
#[cfg(target_os = "linux")]
fn describe_output(stats: &crate::stream::StreamStats, hvc0: bool) -> String {
    use std::fmt::Write as _;

    let [serial, hvc] = stats.received;
    let [serial_dropped, hvc_dropped] = stats.dropped;
    let mut line = format!("{serial} bytes from ttyS0");
    // Writing to a String cannot fail.
    if hvc0 {
        let _ = write!(line, ", {hvc} from hvc0");
    }
    if serial_dropped > 0 || hvc_dropped > 0 {
        let _ = write!(
            line,
            "; dropped because the terminal fell behind: {serial_dropped} ttyS0, {hvc_dropped} hvc0"
        );
    }
    if let Some(e) = &stats.write_error {
        let _ = write!(line, "; {} not shown: {e}", stats.unwritten);
    }
    line
}

/// One line on what the stdin worker did.
#[cfg(target_os = "linux")]
fn describe_input(report: &aegis_kvm::input::InputReport) -> String {
    use aegis_kvm::input::InputEnd;

    let end = match &report.end {
        InputEnd::Eof => "end of file".to_owned(),
        InputEnd::Stopped => "the guest stopping".to_owned(),
        InputEnd::Error(e) => format!("an error: {e}"),
    };
    let held = if report.undelivered > 0 {
        format!(", {} bytes read but not delivered", report.undelivered)
    } else {
        String::new()
    };
    format!("{} bytes forwarded, ended by {end}{held}", report.forwarded)
}

/// The stderr summary: the stop reason that ended the guest (the first vCPU
/// whose run was not stopped on request), then one line per vCPU, the
/// streamed output, the virtio-console's state and the stdin worker's.
#[cfg(target_os = "linux")]
fn summarize(
    normal: bool,
    unassigned: u64,
    results: &[aegis_kvm::smp::VcpuResult],
    output: &str,
    virtio: Option<String>,
    input: Option<String>,
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
        "aegis: guest stopped: {decisive} ({}; {unassigned} unassigned port accesses)",
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
    let _ = write!(summary, "\naegis:   output: {output}");
    if let Some(v) = virtio {
        let _ = write!(summary, "\naegis:   virtio-console: {v}");
    }
    if let Some(i) = input {
        let _ = write!(summary, "\naegis:   stdin to hvc0: {i}");
    }
    summary
}

/// Boot `cfg` and run it until it stops. Every vCPU thread installs the
/// seccomp filter on itself before entering the guest.
#[cfg(target_os = "linux")]
pub(crate) fn run_vm(cfg: &VmConfig) -> Result<RunReport, String> {
    use std::sync::{Mutex, PoisonError};

    use aegis_core::{VmEvent, VmLifecycle, vm_stop_event};
    use aegis_devices::{Serial16550, VirtioConsole, VirtioMmio};

    use crate::stream::{self, OutputStream};

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

    let output = OutputStream::new(stream::CAPACITY);
    let mut uart = Serial16550::with_sink(output.sink(stream::Console::Serial));
    let irq = vm.irq_line(VIRTIO_CONSOLE_GSI);
    let console = cfg.virtio_console.then(|| {
        let device = VirtioConsole::with_sink(
            output.sink(stream::Console::Hvc0),
            VirtioConsole::DEFAULT_INPUT_CAPACITY,
        );
        Mutex::new(VirtioMmio::new(device, &vm, &irq))
    });

    eprintln!(
        "aegis: seccomp run-loop filter installed on every vCPU thread; booting {} MiB guest with {} vCPU(s){}{}",
        cfg.memory_mib,
        cfg.vcpus,
        if cfg.virtio_console {
            " and virtio-console"
        } else {
            ""
        },
        if cfg.virtio_console_stdin {
            "; stdin goes to hvc0"
        } else {
            ""
        }
    );
    let run = run_guest(
        vcpus,
        &mut uart,
        console.as_ref(),
        cfg.virtio_console_stdin,
        &output,
    )?;
    let console = console.map(|c| c.into_inner().unwrap_or_else(PoisonError::into_inner));

    let reasons: Vec<_> = run.results.iter().map(|r| r.outcome.stop.clone()).collect();
    let event = vm_stop_event(&reasons);
    lifecycle.apply(event).map_err(|e| e.to_string())?;
    let virtio_state = console.as_ref().map(|v| {
        format!(
            "status {:#x}, {} bytes in, {} pending, {} KVM_IRQ_LINE failures{}",
            v.status(),
            v.device().delivered_input(),
            v.device().pending_input(),
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
            run.unassigned,
            &run.results,
            &describe_output(&run.output, cfg.virtio_console),
            virtio_state,
            run.input.as_ref().map(describe_input),
        ),
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
