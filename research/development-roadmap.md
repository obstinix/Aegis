# Development Roadmap

- **Date:** 2026-10-26. Implements [`architecture-decision-record.md`](architecture-decision-record.md).
- **Status vocabulary:** `not started` · `in progress` · `done (tested: <where>)` · `blocked (<by what>)`.
- **Per-milestone procedure (directive §11):**
  1. Write acceptance criteria.
  2. Implement.
  3. Add tests.
  4. Run fmt and clippy.
  5. Run `cargo deny` and the security checks.
  6. Build.
  7. Document limitations.
  8. Update this file.
  9. Make focused commits.
  10. Push only after verification **and** owner approval.
- A milestone is `done` only when its acceptance tests pass **in the named environment**. Emulator runs are labeled as emulator runs.

---

## Track H: hosted (KVM)

| # | Milestone | Acceptance criteria | Environment | Depends on | Status |
|---|---|---|---|---|---|
| **M0** | Workspace skeleton | Cargo workspace with `aegis-core` holding real code and tests. Each other ADR-0003 crate is added when its milestone starts, so no empty placeholder crates. Toolchain pinned to 1.97.0. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass on Windows and in WSL2. `deny.toml` allowlist present and `cargo deny check` passes. `aegis-core` builds for `x86_64-unknown-uefi`. | Windows + WSL2 | — | **done (tested: Windows 1.97.0-msvc; WSL2 Ubuntu 24.04 1.97.0-linux-gnu)**. See M0 detail below. |
| **M1** | `aegis doctor` | Reports: CPU vendor/model; VMX/SVM; EPT/NPT and the VMX feature list (Linux: `/proc/cpuinfo` `vmx flags`); `/dev/kvm` presence **and whether it opens**; `KVM_GET_API_VERSION` and key `KVM_CHECK_EXTENSION` results; `kvm_intel` nested/ept/unrestricted_guest; CPU vulnerability status. `--json` emits a versioned schema. Unit tests use fixtures captured from this workstation's WSL2 (including the current "permission denied" case). On Windows it reports "hosted KVM unsupported; Hyper-V active" truthfully. | WSL2, Windows | M0 | **done (tested: Windows 1.97.0-msvc; WSL2 Ubuntu 24.04 1.97.0-linux-gnu, with and without `/dev/kvm` access)**. See M1 detail below. |
| **M2** | KVM: first guest | Creates a VM with 2 MiB of guest RAM; loads `tests/guests/hello16.asm` (real mode: `out 0x3F8` of a known string, then `hlt`); runs one vCPU. Asserts the exact IO-exit sequence and the final `Hlt`. Lifecycle reaches `Destroyed`; all fds closed (checked via `/proc/self/fd`). A negative test: a guest writing to an unassigned port is logged and the VM continues per policy; an unknown exit stops the VM with diagnostics. | WSL2 KVM | M1 | **done (tested: WSL2 Ubuntu 24.04, live KVM; mock backend on Windows and Linux)**. See M2 detail below. |
| **M3** | KVM: boot Linux | A bzImage + initramfs load from paths in `vm.toml` (`aegis run --config`), via the 64-bit boot protocol implemented in `aegis-kvm::boot` (originally planned with `linux-loader`; see ADR-0004 note). In-kernel irqchip + PIT2; 16550 UART. The guest prints a marker from init to the serial console within a timeout. The VMM installs a seccomp allowlist after setup; a test shows a forbidden syscall kills the process. Memory cap enforced. | WSL2 KVM | M2 | **done (tested: WSL2 Ubuntu 24.04, live KVM, WSL2 kernel 6.18.33.2 as the guest)**. See M3 detail below. |
| M4h | KVM: multi-vCPU + virtio | 2+ vCPUs boot Linux, discovered through ACPI (MADT). A virtio-console over virtio-mmio, declared in the ACPI DSDT, carries guest output through a split virtqueue with interrupt delivery. Malformed queues and registers are handled without host panics (unit-tested). *Scope adjusted 2026-10-26; see the M4h scope note below.* | WSL2 KVM | M3 | **done (tested: WSL2, live KVM, WSL2 kernel 6.18.33.2 as the guest, 1/2/4/8 vCPUs)**. See M4h detail below. |
| M5h | PyxisOS Track A validation | M3 tests rerun on the ThinkPad X1 Carbon (i7-7600U). Results are recorded in `compatibility-matrix.md` as hardware-tested. | Physical hardware | M3; owner runs it on that machine | **blocked (owner runs it on the ThinkPad)**. The script and checklist are ready: `scripts/hardware-validation.sh`, `docs/hardware-validation.md` (2026-10-27). Nothing has run on that machine yet. |
| M6h | KVM: virtio-console input | Bytes on `aegis run`'s stdin reach a Linux guest's `/dev/hvc0` through the virtio-console receive queue, and the output path is unchanged. Input buffering is bounded, with backpressure. Malformed receive chains are handled without host panics (unit-tested). The host input worker cannot outlive the run. | WSL2 KVM | M4h | **done (tested: WSL2, live KVM, WSL2 kernel 6.18.33.2 as the guest)**. See M6h detail below. |

### M0 detail (2026-10-26)

Windows results were rerun from a clean build on the pinned `1.97.0-x86_64-pc-windows-msvc` toolchain (linker from VS 2022 Build Tools). Earlier runs used `stable-x86_64-pc-windows-gnu` (also rustc 1.97.0) with the same results. WSL2 results are in the last row.

| Criterion | Result |
|---|---|
| Workspace + `aegis-core` (lifecycle, guest memory map, VM-exit model) | done |
| `cargo test --workspace` | **passed** on Windows: 22 unit tests, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` (pedantic + `arithmetic_side_effects`) | **passed** on Windows |
| `cargo fmt --all --check` | **passed** |
| `cargo doc` with `-D warnings` | **passed** |
| `no_std` proof: build and clippy for `x86_64-unknown-none` (installed target) | **passed** |
| Build for `x86_64-unknown-uefi` | **passed** (debug + release + clippy `-D warnings`). Target added with `rustup target add x86_64-unknown-uefi` on the stable toolchain, owner-approved 2026-10-26. |
| Toolchain pin (`rust-toolchain.toml`) | **done**: `channel = "1.97.0"`, with clippy, rustfmt, and the `x86_64-unknown-uefi` and `x86_64-unknown-none` targets. Owner-approved download 2026-10-26. On Windows it resolves to the MSVC host toolchain. |
| `cargo deny check` | **passed**: advisories, bans, licenses and sources all ok, with no warnings. `cargo-deny` 0.20.2 installed via `cargo install --locked`, owner-approved 2026-10-26. `deny.toml` holds the ADR-0011 allowlist. Negative test: a scratch copy declaring `GPL-3.0-only` fails with `rejected: license is not explicitly allowed` (exit 4). With no dependencies yet, this proves the policy is wired, not that any dependency was vetted. |
| Same checks in WSL2 | **passed**, all of them, on `1.97.0-x86_64-unknown-linux-gnu` (WSL2 Ubuntu 24.04, kernel 6.18.33.2): fmt; tests 22/22; clippy `-D warnings` on host, none and uefi; docs `-D warnings`; debug and release builds for both `no_std` targets; `cargo deny check`. rustup 1.29.2 was installed from the official `rustup-init` binary, SHA-256 verified against `static.rust-lang.org` (`--profile minimal --default-toolchain none`; PATH line added to `~/.profile` and `~/.bashrc`). The toolchain was installed with `--no-self-update`. `cargo-deny` 0.20.2 was installed with `--locked`. Owner-approved 2026-10-26. Linux build output goes to `~/aegis-target` (`CARGO_TARGET_DIR`) so it doesn't share `target/debug` with Windows builds. |

### M1 detail (2026-10-26)

| Criterion | Result |
|---|---|
| CPU vendor/model, VMX/SVM, hypervisor present + vendor (CPUID) | **done**: live on Windows and WSL2; fixture-tested against `/proc/cpuinfo` and Phase 1 WMI values |
| EPT/NPT and VMX feature list | **done** on Linux, from `vmx flags`. Windows reports `unknown`: EPT is in a ring-0-only MSR, and the report says so instead of guessing. |
| `/dev/kvm` presence and whether it opens | **done**: real read-write open; "permission denied" detected live in WSL2 |
| `KVM_GET_API_VERSION`, `KVM_CHECK_EXTENSION` | **done**: ran live in WSL2 after the owner added `obsti` to `kvm`. API version 12; `irqchip`, `user_memory`, `pit2`, `ext_cpuid`, `immediate_exit` all present. Cross-checked with raw `fcntl.ioctl` calls from Python (no `kvm-ioctls`, no Aegis code), which gave identical values. Captured as fixture `wsl2-ultra9-275hx-kvm` with 3 tests. Both verdicts are READY. The nested one is a capability claim; no nested guest has run. |
| `kvm_intel` nested/ept/unrestricted_guest (+ enable_apicv) | **done** |
| CPU vulnerability status | **done** (Linux, passed through verbatim). Windows: not collected, and the report says so. |
| `--json` with a versioned schema | **done**: `aegis.doctor/1`; shape pinned by a test. `--raw` emits the observations used for fixtures. |
| Fixture tests from this workstation (WSL2 incl. permission denied; Windows) | **done**: 8 fixture tests; byte-exact captures, provenance in `crates/aegis-probe/tests/fixtures/PROVENANCE.md`. Mutation check: setting the fixture's `nested` to `N` fails exactly the 3 dependent tests. |
| Windows reports hosted KVM unsupported, Hyper-V active | **done**: "UNSUPPORTED" plus a note that Hyper-V (`Microsoft Hv`) hides VMX from the OS |
| Checks | fmt; 47/47 tests; clippy `-D warnings` (Windows host, Linux host, `no_std` none + uefi for `aegis-core` and `aegis-probe`); docs `-D warnings`; `cargo deny check`. All pass on Windows (1.97.0-msvc) and WSL2 (1.97.0-linux-gnu). |
| New dependencies | `serde` 1.0.229, `serde_json` 1.0.154 (pulls `zmij`, `itoa`, `memchr`), `kvm-ioctls` 0.25.1 (Linux only; pulls `kvm-bindings`, `vmm-sys-util`, `libc`, `bitflags` 1 + 2). 17 external crates, all MIT / Apache-2.0 / BSD-3-Clause. `cargo deny` warns about the duplicate `bitflags`, which comes from upstream `vmm-sys-util`. |

### M2 detail (2026-10-26)

| Part | Status |
|---|---|
| Backend interface (`Vcpu`, `PortIo`) and dispatch loop (`run_vcpu`) in `aegis-core` | **done**: tested with the scripted mock backend. Covers normal stop, shutdown, unassigned ports, unknown exit, unhandled MMIO, backend error, exit limit and refusal when not running. 8 tests. |
| Port bus (`PortBus`) with the unassigned-port policy | **done**: 4 unit tests (routing, all-ones reads, logging cap, range validation) |
| 16550 UART (`aegis-devices`, transmit-only, output capped) | **done**: 5 unit tests + 1 bus/mock integration test |
| NASM test guests `hello16`, `unassigned_port`, `mmio_unmapped` (`tests/guests/`) | **done**: assembled at test time with NASM 2.16.01 `-w+all` and run under KVM |
| `aegis-kvm` (KVM backend, real-mode vCPU setup, guest memory via `vm-memory` 0.18.0) | **done**: one `unsafe` block (`KVM_SET_USER_MEMORY_REGION`) under crate-wide `deny(unsafe_code)`. Its safety rests on drop order (VM fd before memory) and `KvmVcpu<'vm>` borrowing the VM. Limits: real mode only; string port I/O is reported as unhandled; memory cannot be made non-executable (refused rather than ignored). 2 unit tests. |
| KVM acceptance tests (`crates/aegis-kvm/tests/kvm_guests.rs`) | **done**, 5 tests, passing on live KVM in WSL2. Covered: `hello16` with the exact exit sequence (33 port writes to 0x3F8, then HLT) and UART output; lifecycle to `Destroyed`; no fd leak (`/proc/self/fd`); unassigned ports (all-ones read reaches the guest, both accesses logged, guest continues); unmapped MMIO (VM fails with the access recorded); out-of-bounds host writes; non-executable memory refused. 4 of them are `#[ignore]`d because they need `/dev/kvm`, so a plain `cargo test` reports them as ignored. Mutation checks: changing one character of the guest message fails `hello16`; leaking the VM fails the fd check (7 vs 5). **Gap:** a truly unknown KVM exit is covered only by the mock backend; no guest triggers one live. |
| Checks | WSL2: fmt; 75/75 tests including the live KVM tests; clippy `-D warnings` (Linux host, plus `no_std` none and uefi for `aegis-core`, `aegis-devices`, `aegis-probe`); docs `-D warnings`; `cargo deny check`. Windows: the same, 68/68 tests (`aegis-kvm` is empty there). Docs on Windows initially failed with broken intra-doc links in `aegis-kvm` (crate docs render on Windows, its types don't); fixed. |

### M3 detail (2026-10-26)

| Part | Status |
|---|---|
| 64-bit boot protocol (`aegis-kvm::boot`) | **done**, with 8 unit tests on synthetic images. It parses and validates the bzImage (protocol ≥ 2.12, `LOADED_HIGH`, `XLF_KERNEL_64`, bounded header). It builds the zero page by copying the setup-header bytes and patching loader fields, with a 2-entry e820 map, the initramfs placed high and page-aligned, a 1 GiB identity map, and a GDT with `__BOOT_CS` 0x10 / `__BOOT_DS` 0x18 as the protocol requires. No `unsafe`. |
| KVM setup (`KvmVm::enable_legacy_interrupts`, `load_linux`, `KvmVcpu::set_linux_entry`) | **done**: TSS address, in-kernel PIC/IOAPIC and PIT2 (only callable before any vCPU exists, enforced by `&mut self`); supported CPUID; LAPIC LINT0 `ExtINT` / LINT1 NMI; long-mode sregs; RIP at the 64-bit entry, RSI at the zero page. |
| `aegis run --config vm.toml` | **done**: strict TOML (unknown keys rejected, size cap, 16–1024 MiB). Guest serial output goes to stdout with control bytes escaped; a summary goes to stderr; exit 0/1/2. 6 config and escape unit tests. |
| Seccomp run-loop allowlist (`aegis-kvm::seccomp`) | **done**: kill-process on mismatch. `ioctl` only for `KVM_RUN`; `write` only to fds 1 and 2; `fcntl` only `F_GETFD`; memory, futex and teardown syscalls. `aegis selftest seccomp` attempts `openat` and is killed by `SIGSYS` (test). |
| Linux boot test (`crates/aegis-cli/tests/boot_linux.rs`) | **done**, passing on live KVM. A NASM `/init` (`tests/guests/init_marker.asm`) in a Rust-generated newc initramfs writes `AEGIS_M3_INIT_OK` to `/dev/kmsg` and restarts. Asserted: the filter is installed, the kernel banner and the marker appear on the UART console, and the stop is `Shutdown` with exit 0. About 2.5–3 s per boot, 263 console lines, ~75k exits. |
| Mutation checks | Changing the marker in `/init` fails the marker assertion. Making seccomp allow a different ioctl than `KVM_RUN` gets the boot killed by `SIGSYS`, which proves the filter enforces during the boot. |
| Checks | WSL2: 92/92 tests including live KVM and Linux boot; Windows: 73/73. fmt, clippy (zero findings), docs, `no_std` builds, `cargo deny` pass on both. |

**Known limits (stated, not hidden):**
- The UART raises no interrupts. The kernel console works because the 8250 console path polls LSR, but userspace writes to `/dev/ttyS0` would stall waiting for a THRE interrupt. That's why `/init` uses `/dev/kmsg`.
- One vCPU; no ACPI or MP tables; `pci=off`.
- Serial output is printed after the run, not streamed.
- The guest kernel is the WSL2 kernel binary (GPL-2.0, Microsoft build) used locally as test input. It is not distributed with Aegis.

### M4h scope note (2026-10-26)

The original M4h row asked for virtio-blk and fuzz targets. The test guest kernel's build config (WSL2 6.18.33.2, `/proc/config.gz`) changes what can be integrated and tested:

| Kernel option | Value | Effect on M4h |
|---|---|---|
| `CONFIG_X86_MPPARSE` | not set | The kernel ignores Intel MP tables, so SMP CPUs must be described in **ACPI** (RSDP → XSDT → FADT/DSDT/MADT). |
| `CONFIG_VIRTIO_MMIO` | y | The virtio-mmio transport is available. |
| `CONFIG_VIRTIO_MMIO_CMDLINE_DEVICES` | not set | virtio-mmio devices cannot be declared on the kernel command line, so they are declared in the **ACPI DSDT** as `LNRO0005` devices (the virtio-mmio driver's ACPI ID). |
| `CONFIG_VIRTIO_BLK` | m (module) | virtio-blk cannot be tested under `nomodules` with this kernel. **Deferred.** |
| `CONFIG_VIRTIO_CONSOLE` | y | **virtio-console is the M4h device**: built in, the smallest complete virtio device (two split virtqueues), and it gives userspace an interrupt-driven console. |
| `CONFIG_VIRTIO_PCI` | y | Would need PCI config-space and BAR emulation; out of proportion for M4h. |

Fuzz targets move to a later milestone. Installing `cargo-fuzz` needs a nightly toolchain, which this workspace does not pin. The device register and queue paths get adversarial unit tests instead.

### M4h detail (2026-10-26)

| Part | Status |
|---|---|
| Core: MMIO dispatch and stopping from outside (`aegis-core`) | **done**. `GuestIo` = `PortIo` + `MmioIo`; `DeviceBus` (was `PortBus`) with port and MMIO windows; `VmExit::MmioDevice` (resumable). `run_vcpu_until` with a `StopFlag` and `StopReason::Requested`; `vm_stop_event` combines per-vCPU reasons. Mock: scripted MMIO and interruptions, `VecMemory`, `RecordingLine`. 41 tests (was 34). |
| Split virtqueue (`aegis-devices::virtio::queue`) | **done**, `no_std`. Ring layout and alignment checks; ring and descriptor indices validated; chain walk bounded by the queue size; 1 MiB chain cap; indirect descriptors rejected; checked address arithmetic; Acquire/Release fences around the ring indices; `VRING_AVAIL_F_NO_INTERRUPT` honored. 6 tests, including malformed rings. |
| virtio-mmio transport (`aegis-devices::virtio::mmio`) | **done**. Version 2 registers. 32-bit accesses only. Status bits can only be added, and writing 0 resets. `FEATURES_OK` needs `VIRTIO_F_VERSION_1` and no unoffered bits; `DRIVER_OK` needs `FEATURES_OK`. Queue fields are frozen while ready, and re-enabling a ready queue is ignored (it would replay returned buffers). A malformed queue sets `DEVICE_NEEDS_RESET` with a config interrupt. `InterruptStatus`/`InterruptACK` drive a level-triggered line. 10 tests, which follow Linux's init sequence. |
| virtio-console (`aegis-devices::virtio::console`) | **done for output**. Device ID 3, no features, port 0 only. The transmit queue's readable buffers go to a 64 KiB capped buffer (overflow counted); the receive queue is accepted but never filled. |
| ACPI tables (`aegis-kvm::acpi`) | **done**: RSDP, XSDT, FADT (HW-reduced), DSDT, and MADT at `0xE_0000`, 16-byte aligned, using `acpi_tables` 0.2.1. The DSDT has one `LNRO0005` device per virtio-mmio window, each with a unique `_UID`. 7 unit tests parse the bytes back: checksums sum to zero, lengths, RSDP→XSDT→FADT→DSDT links, MADT entries, and the AML `_HID`/`_UID`/`_CRS` encodings. The RSDP goes in `boot_params.acpi_rsdp_addr` (protocol ≥ 2.14, unit-tested). |
| SMP runner (`aegis-kvm::smp`) | **done**: one scoped thread per vCPU, each sandboxed; a shared `Mutex<DeviceBus>`; stop flag plus a raw-`tgkill` `SIGRTMIN` kick, re-sent every 10 ms. APs get CPUID with their own APIC ID (unit-tested) and wait for INIT/SIPI; `KVM_RUN`'s `EAGAIN` after a state change is retried. |
| `aegis run` | **done**: `vcpus = 1..8` (default 1) and `virtio_console = true/false` (default false) in `vm.toml`. hvc0 output is printed after the UART output under `--- virtio-console (hvc0) ---`, escaped. The stderr summary names the decisive stop and gives one line per vCPU plus the virtio state. |
| Live test (`boots_two_vcpus_with_a_virtio_console`) | **done**, passing on live KVM in about 2.5–3 s. A NASM `/init` (`tests/guests/init_m4h.asm`) mounts `/proc`, writes a marker to `/dev/hvc0`, and copies `/proc/interrupts` to it. Asserted: the kernel uses the MADT and brings up 2 CPUs; no ACPI warning or error; both hvc0 markers; the guest's own interrupt table has `CPU0 CPU1` columns and a nonzero `virtio0` count; vCPU 0 stops with `Shutdown`, the other with `Requested`; 0 `KVM_IRQ_LINE` failures; no `DEVICE_NEEDS_RESET`; exit 0. Also run by hand: 15/15 repeated boots; 4 and 8 vCPUs boot with all CPUs online. |
| Guest-side evidence (from the boot log, not assumed) | The kernel lists every table at the address the builder placed it. "ACPI: Using ACPI (MADT) for SMP configuration information"; "IOAPIC[0]: … GSI 0-23"; "ACPI: 1 ACPI AML tables successfully acquired and loaded"; `virtio0` on `IO-APIC 5-fasteoi` (level-triggered). With `acpi_force_table_verification`, the kernel checked every checksum at boot and reported none. |
| Mutation checks | All on scratch copies, none committed. Each fails the test or the run: a corrupted MADT checksum (caught only after adding the "no ACPI warning" assertion, since the kernel warns but still uses the table); no used-buffer interrupt (the interrupt count is 0); no kick (the VMM hangs with the sibling halted in `KVM_RUN`, still running at a 30 s limit); `pthread_kill` instead of a raw `tgkill` (killed by `SIGSYS`); no `EAGAIN` retry (`KVM_RUN` error on vCPU 1 after 0 exits). The last two were real bugs found by the first live runs. |
| Checks | WSL2: 127/127 tests including live KVM and both Linux boots (121 plus 6 ignored without `--include-ignored`). Windows: 97/97. fmt, clippy (zero findings), docs, `no_std` none/uefi builds, and `cargo deny` pass on both. |

**Known limits (stated, not hidden):**
- virtio-console is output-only: there is no host input, and the receive queue stays empty. There is no `VIRTIO_CONSOLE_F_SIZE`, no multiport, and no `EVENT_IDX`. *(Input was added in M6h.)*
- Device emulation runs on the vCPU thread, under a single bus lock. That's fine for a console, not a design for fast devices.
- Guest output is still printed after the run, not streamed.
- At most 8 vCPUs (the MADT builder's limit). There's no CPU hotplug, NUMA or topology leaves beyond the APIC ID. The vCPUs are not pinned.
- The summary shows the device status at stop, which is 0. The guest's last write to `Status` was a reset; since the queues only work after `DRIVER_OK`, that reset came after all output, on the way to reboot.
- The ACPI tables are checked by unit tests and by one guest kernel (Linux 6.18 via ACPICA). No other OS or firmware has consumed them, and no ACPI compiler (`iasl`) was run on them.
- No fuzzing (see the scope note).

### M6h detail (2026-10-26)

Host input to the virtio-console, so a host-provided byte stream reaches the guest's `/dev/hvc0`. Aegis still runs guests on KVM; nothing here is bare metal.

| Part | Status |
|---|---|
| Receive path (`aegis-devices::virtio::console`) | **done**, `no_std`, no `unsafe`. `queue_input` puts host bytes into a bounded FIFO (default 4 KiB) and returns how many it took; the caller keeps the rest (backpressure). Bytes are written in order into the driver's device-writable receive chains, each chain filled before the next. The last chain may be partly filled; its used length is the number of bytes written. A buffer is taken from the ring only when there is input for it. Each call makes at most one pass over the ring. A receive chain containing a device-readable descriptor is rejected (`QueueError::NotWritable`, then `DEVICE_NEEDS_RESET`). Bytes leave the FIFO only after the guest-memory write succeeds, so a failed write loses nothing. 13 new unit tests. |
| Port gate | **done**. Input waits for the guest's first transmit on the port; then the receive queue is processed in the same exit. A reset closes the gate again. *Why:* Linux's non-multiport driver posts its receive buffers in `add_port()` (one kick each) before it puts the port on its list, and `in_intr()` frees any buffer that comes back for a port it cannot find. Verified in the kernel source and live: without the gate, all 32 bytes were written into guest buffers and none reached userspace. |
| Transport (`aegis-devices::virtio::mmio`) | **done**. `process(index)` lets the host start processing, under the same rules as a guest notification (`DRIVER_OK`, no `DEVICE_NEEDS_RESET`, queue ready). `device_mut()` gives access to the device. A device can request one follow-up queue (`VirtioDevice::take_requested_queue`), bounded by the number of queues. |
| Host input worker (`aegis-kvm::input`) | **done**. It waits with `poll` (20 ms timeout) and reads at most 1 KiB at a time, and only after `poll` reports data. Bytes the device does not take are offered again every interval; nothing more is read meanwhile. It ends at end of file, on an error, or on the stop flag, and reports bytes forwarded and bytes undelivered. Two `unsafe` libc calls (`poll`, `read`), each with a SAFETY comment. 6 unit tests over Unix socket pairs: order across chunks, backpressure, prompt stop, nothing delivered after stop, undelivered count, and an invalid descriptor reported as an error rather than a hang. |
| Worker sandbox (`seccomp::input_worker_filter`) | **done**: the run-loop allowlist plus `read` on fd 0 and `poll`/`ppoll`, nothing else. A unit test checks that exactly those three are added, that every run-loop rule is unchanged, and that vCPU threads still cannot `read`. |
| SMP runner (`aegis-kvm::smp`) | **done**. The stop flag is now provided by the caller. `SharedMmio` puts a device behind its own mutex: vCPUs always take the bus lock and then the device lock, and the worker takes only the device lock, so the two cannot deadlock. `HostThread` registration: once the guest stops, the runner kicks the worker with `SIGRTMIN` and waits for it like a vCPU. That covers a `read` blocked because another process drained the shared stdin between `poll` and `read`. 1 unit test kicks a thread out of a blocked `read`. |
| DMA permissions (`GuestMemoryAccess for KvmVm`) | **done**, found in this milestone's security review. The receive path is the first device code that writes guest memory, and its accessor went through the image-loading path, which ignores guest permissions. Device reads and writes now follow the memory map's read/write permissions. Live test with an RWX region and an RX region. No effect on the Linux boot, whose map is one RWX region. |
| `aegis run` / `vm.toml` | **done**: `virtio_console_stdin = true` (default false; requires `virtio_console = true`). The summary now reports bytes out, bytes in and pending input, plus a `stdin to hvc0:` line saying how the worker ended. 1 config test. |
| Live test (`guest_reads_host_stdin_from_hvc0`) | **done**, about 3 s on live KVM. A NASM `/init` (`tests/guests/init_console_input.asm`) writes `READY` to hvc0, then reads two lines (with a 10 s `poll` timeout each, so a failure ends the run instead of hanging) and writes each back framed as `AEGIS_RX[...]`. The tty echoes input on its own, but never with that framing. Two vCPUs. Stdin is held **open** after the input, so the run ends only if the guest's stop also stops the worker. Asserted: both lines arrive in order, byte for byte; 32 bytes in and 0 pending; `ended by the guest stopping`; `Shutdown`; no needs-reset; 0 `KVM_IRQ_LINE` failures; exit 0. |
| Manual live checks | Immediate end of file (`printf … \| aegis run`): both lines read; the worker `ended by end of file`. Late input, sent about 2.5 s and 3.5 s after launch (after `/init` started): both lines read, through the host-initiated path rather than the gate. |
| Mutation checks | On scratch copies, none committed. With no port gate, the live test fails exactly as before the fix (32 bytes in, guest timeout). With a worker that ignores the stop flag, `aegis` is still running at a 20 s limit, where a normal run takes about 3 s. With the DMA write-permission check removed, the permissions test fails. |
| Checks | Windows: 111/111 (M4h: 97; +13 console, +1 config). WSL2: 151/151 including 8 live tests (M4h: 127 with 6 live); a plain `cargo test` runs 143 and skips the 8 live ones. The WSL2 additions: +13 console, +1 config, +6 worker, +1 filter, +1 kick, +1 live input boot, +1 live permissions test. fmt, clippy (zero findings), docs, `no_std` none/uefi builds and `cargo deny` pass on both. No new dependencies. |

**Known limits (stated, not hidden):**
- The guest receives input only after it has written to hvc0 (the port gate). A guest that reads before it ever writes gets nothing. A getty or shell prompt, or this test's `/init`, writes first. The protocol-level fix is `VIRTIO_CONSOLE_F_MULTIPORT`, whose control queue reports `PORT_READY`/`PORT_OPEN`; it is not implemented.
- With `console=hvc0`, kernel messages could open the gate during the driver's probe, bringing the race back. The default command line uses `console=ttyS0`.
- Not an interactive terminal: stdin is forwarded as raw bytes, with no raw mode and no Ctrl-C passthrough, and the guest's tty echoes the input into its output. When M6h landed, guest output was also printed only after the run; it is streamed since 2026-10-27 (see "Cross-cutting work" below), but that alone does not make the console interactive.
- Input not delivered when the guest stops is discarded and reported (`pending`, `read but not delivered`).
- One console port; no `VIRTIO_CONSOLE_F_SIZE`, no `EVENT_IDX`, no fuzzing.
- M5h (hardware validation on the ThinkPad) is still not started; nothing here was run on that machine.

**Deviations from the ADRs:** see the ADR-0007 and ADR-0008 M6h notes (`smp::run` signature; the port gate; DMA permissions). The two new `unsafe` blocks follow ADR-0002: backend crate only, each with a SAFETY comment.

**Remaining work toward B0 (not started).** The prerequisites are present in WSL2 (checked 2026-10-26): `qemu-system-x86_64`, OVMF (`/usr/share/OVMF/OVMF_CODE_4M.fd`), `kvm_intel` with `nested=Y`, and the pinned `x86_64-unknown-uefi` target. Still to build:
1. An `aegis-uefi` crate that prints a marker over serial.
2. `cargo xtask uefi-test`, which builds it, makes a FAT image (tool not chosen; nothing installed), boots it in QEMU with OVMF, captures serial to a file, asserts the marker and enforces a timeout.

M6h neither depends on B0 nor changes it.

## Track B: bare metal (VMX, UEFI)

| # | Milestone | Acceptance criteria | Environment | Depends on | Status |
|---|---|---|---|---|---|
| **B0** | Nested test harness | `cargo xtask uefi-test` builds `aegis-uefi.efi`, creates a FAT image, boots it in WSL2 `qemu-system-x86_64 -enable-kvm -cpu host -bios OVMF`, captures serial to a file, asserts a "hello" marker, and enforces a timeout. Failure output includes the serial log. | WSL2 QEMU/KVM | M0 | **done (tested: WSL2, QEMU 8.2.2 with KVM nested in WSL2, OVMF 2024.02)**. See B0 detail below. |
| **B1** | VMX capability + VMXON | In the L2 guest: checks CPUID.1:ECX.VMX and `IA32_FEATURE_CONTROL` (lock/enable bits); reads `IA32_VMX_BASIC` and the true-controls MSRs; applies the CR0/CR4 fixed bits; VMXON; VMXOFF; reports each step over serial. Clean failure paths are tested by forcing each check to fail (unit-tested decision logic over captured MSR values). | WSL2 nested | B0 | not started |
| **B2** | First guest | VMCS with deny-by-default MSR and I/O bitmaps; EPT built from a guest memory map (ADR-0006) and walked by a test to prove hypervisor pages are absent; a guest running a few instructions → `CPUID` exit (handled) → `HLT` exit (stop). VM-entry failures report the VM-instruction error. | WSL2 nested | B1 | not started |
| **B3** | Shared interfaces | `aegis-uefi` implements the ADR-0007 `Vm`/`VmExit` model; the same `aegis-core` dispatch tests run against both backends' exit streams | Any + nested | B2, M2 | not started |
| B4 | Serial device + I/O exits | The bare-metal guest writes to the 16550 via I/O exits, served by `aegis-devices` (shared with the hosted backend) | WSL2 nested | B3 | not started |
| B5 | Boot a guest kernel | Load a minimal 64-bit test kernel (later Linux) after `ExitBootServices`; long-mode guest state setup | WSL2 nested | B4 | not started |
| B6 | AMD SVM | Verify QEMU TCG SVM support first; then VMRUN + NPT with the B1/B2 criteria | QEMU TCG (to verify) | B2 | not started |
| B7 | Physical hardware | B2 criteria on the ThinkPad (i7-7600U) from a USB EFI boot | Physical hardware | B2; owner | not started |

### B0 detail (2026-10-26)

The bare-metal track's test harness. The application it boots does no virtualization yet; VMX is B1.

| Part | Status |
|---|---|
| `aegis-uefi` (`crates/aegis-uefi`) | **done**. A `no_std`/`no_main` UEFI application (`aegis-uefi.efi`, 7.5 KiB release, 14 KiB debug). It prints `AEGIS_B0_HELLO`, the UEFI revision and the firmware revision on the firmware console, then powers the machine off with `ResetSystem(EfiResetShutdown)`. Firmware access goes through `uefi-raw` types, with three `unsafe` blocks (system-table dereference, `OutputString`, `ResetSystem`), each with a SAFETY comment. On other targets it builds as a stub that explains itself, so `cargo test --workspace` works everywhere. |
| FAT image (`xtask/src/fat.rs`) | **done**. A deterministic 16 MiB FAT16 superfloppy (no partition table) holding only `\EFI\BOOT\BOOTX64.EFI`, laid out per Microsoft's FAT specification. It is written by `xtask` because no FAT tools (`mtools`, `dosfstools`) are installed and none are needed. 6 unit tests parse it back using layout values read from the image itself: FAT16 cluster count, fatgen103's FAT-size formula, byte-for-byte read-back across cluster boundaries, the maximum size and one byte over, an empty file, `.`/`..` links, both FAT copies identical, determinism. |
| Harness (`cargo xtask uefi-test`) | **done**. The steps:<br>1. Check prerequisites; a missing one (not Linux, `/dev/kvm` not openable, no QEMU, no OVMF) is reported as **SKIPPED**, exit 77, never a pass.<br>2. Build the EFI app and write the image.<br>3. Run `qemu-system-x86_64 -nodefaults -machine q35 -enable-kvm -cpu host -smp 1 -m 256M -display none -no-reboot -bios /usr/share/ovmf/OVMF.fd`, with the image read-only on virtio-blk and serial to a file.<br>4. Pass only if the marker is in the serial log **and** QEMU exited by itself within the timeout (default 60 s, `--timeout`).<br>Any failure prints the reason, the QEMU command, the escaped serial log and QEMU's stderr. A stale log is deleted first. 6 unit tests cover argument parsing, the verdict logic, the QEMU command line, firmware lookup, log escaping and finding the artifact in cargo's JSON. |
| Live run | **PASSED** in about 0.6 s of QEMU time (about 6 s with the build); 10/10 repeated runs passed. OVMF's boot manager loaded `BOOTX64.EFI` from the image (`BdsDxe: starting Boot0001 … PciRoot(0x0)/Pci(0x1,0x0)`), and the app reported UEFI 2.70 (`0x00020046`). This is nested virtualization: the app ran under QEMU/KVM inside the WSL2 VM. On Windows, `uefi-test` reports SKIPPED (exit 77). |
| Failure paths (mutation checks) | On scratch copies, none committed. Each run failed with exit 1, the reason and the serial log, and no QEMU process was left running:<br>- Wrong marker text: `QEMU exited normally, but AEGIS_B0_HELLO is not in the serial log`.<br>- No power-off: `appeared, but the VM did not power off within 8 s`; QEMU was killed at the deadline.<br>- Boot file misnamed in the image: OVMF logs `failed to load Boot0001 … Not Found` and starts its shell, and the harness times out. That shell mapped the image as `FS0:`, which is independent evidence that the FAT16 volume is valid. |
| Reproducible builds | **done (same host)**. `.cargo/config.toml` links UEFI builds with `/Brepro` (no timestamp) and `/DEBUG:NONE` (no per-build PDB reference). Two builds in different target directories, and a later rebuild, give a byte-identical `aegis-uefi.efi` and `esp.img`. Builds on Windows and Linux still differ from each other. This was checked by hand; it is not an automated test. *Automated since 2026-10-27: `cargo xtask repro-test`, also run in CI (see "Cross-cutting work").* |
| Checks | Windows: 123/123 (M6h: 111; +12 `xtask` tests). WSL2: 163/163 including 8 live tests (M6h: 151); a plain `cargo test` runs 155 and skips the 8 live ones. Plus `cargo xtask uefi-test`. fmt, clippy (zero findings, including `aegis-uefi` for the UEFI target, debug and release), docs, the `no_std` builds and `cargo deny` pass on both. |

**Deviations (recorded in the ADR-0003 and ADR-0010 notes):**
- `uefi-raw` instead of the `uefi` crate, for licence reasons.
- The FAT image is written by `xtask`.
- A pass also requires a clean power-off, which is stricter than the criterion.
- Skips exit with 77.

**Known limits:**
- No VMX, no memory allocation, no `ExitBootServices`; the app only reports and powers off.
- A panic spins, leaving the harness to time out, because the panic handler has no console.
- The serial marker depends on OVMF mirroring its console to the serial port, as it does by default.
- QEMU option syntax cannot carry commas, so a path containing one is rejected.
- No PDB symbols for the EFI app (the price of reproducible builds).
- No CI. *(Added 2026-10-27; `uefi-test` itself stays local-only.)*

## Cross-cutting work (2026-10-27)

This work belongs to no single milestone: CI, an automated reproducible-build check, streamed console output, the M5h validation script, and fuzzing. The owner's directive of 2026-10-27 set the priorities. Aegis still runs guests only on KVM; none of this is bare-metal virtualization.

| Part | Status |
|---|---|
| CI (`.github/workflows/ci.yml`) | **Written and run locally; not yet run on GitHub.**<br>- `check` job on `ubuntu-24.04` and `windows-2025`: fmt; clippy `--workspace --all-targets --all-features -D warnings`; `cargo test --workspace --all-features`; docs with `-D warnings`; the `no_std` and UEFI builds; `cargo xtask repro-test`; the fuzz smoke tests.<br>- `deny` job: cargo-deny 0.20.2, installed `--locked` and cached.<br>- Hardening: only GitHub's own actions, pinned to commit SHAs; a read-only token; no persisted checkout credentials; no `RUSTFLAGS`, which would replace the UEFI link flags.<br>- The live KVM tests and `uefi-test` stay local (ADR-0010).<br>- Every `run:` step was executed from the parsed workflow on Windows and in WSL2. |
| `cargo xtask repro-test` | **done**. It builds `aegis-uefi.efi` twice, debug and release, each into a freshly emptied target directory, and requires the EFI files and their FAT images to be byte-identical. On a mismatch it names the first differing offset. Mutation check: with `RUSTFLAGS` set, which replaces `/Brepro /DEBUG:NONE`, it fails on both profiles at byte 0x80 (the PE timestamp). 1 new unit test. |
| Streamed console output (`aegis run`) | **done**. Before, output was printed after the guest stopped and capped at 64 KiB per console. The new path:<br>- The devices write to an `OutputSink` (`aegis-core`); the default remains the 64 KiB `CappedBuffer`.<br>- `aegis run` plugs in a sink that feeds one bounded queue (1 MiB, in arrival order).<br>- A writer thread, under the run-loop seccomp filter, sleeps on a condition variable and flushes after each batch, so prompts appear at once. UART output is written as it is; hvc0 lines are prefixed `hvc0| `, or `hvc0+ ` after a cut; control bytes are escaped.<br>- vCPUs never wait for the terminal: overflow is dropped and counted. A failing stdout is drained and the bytes not shown are counted.<br>- A guard stops the input worker and closes the stream if the run unwinds.<br>The config format is unchanged. Tests: 8 stream unit tests, 3 device tests, a parser test for the split stdout, and a live test that sees the guest's hvc0 prompt **while `aegis` is still running**, and only then sends the input that answers it. Checked by hand: `aegis run … \| head -3` stops the guest normally, exits 0, and reports "N not shown: Broken pipe". |
| M5h script (`scripts/hardware-validation.sh`, `docs/hardware-validation.md`) | **Ready; not run on the ThinkPad.** It records the environment, runs the M2/M3 tests as REQUIRED steps and M4h, M6h, streaming, `uefi-test` and `repro-test` as EXTENDED steps, reports missing prerequisites as SKIP, and writes `results.md`. Checked under WSL2 only (all steps passed; skip paths work), and that `results.md` says it ran in a VM. |
| Fuzzing (`fuzz/`, `docs/fuzzing.md`) | **Harnesses done; short runs only.**<br>- Three targets (virtqueue, virtio-mmio registers with the console, `vm.toml`), with invariants asserted on every input.<br>- They live in a separate workspace, excluded from normal builds.<br>- On stable there are deterministic smoke tests and a crash-input replayer, and CI runs them.<br>- Coverage-guided: one 3-minute run per target on Windows, using the nightly toolchain and MSVC AddressSanitizer runtime already installed (cargo-fuzz is not installed). No crashes: 3.8 M, 1.5 M and 0.5 M executions.<br>- Mutation check: with the descriptor-index check weakened, the fuzzer found the bug in about 110 executions, and the saved input replayed the failure on stable.<br>- Windows needs a panic hook to save crash inputs, because a Rust abort there is a fast-fail that libFuzzer cannot catch. |
| `ucs2` licence question | Inspected; options written up in the licensing review §3.1 for the owner. `deny.toml` unchanged. |
| Checks | Windows: 134/134 (B0: 123; +8 stream, +3 devices, +1 `xtask`, −1 removed escape test). WSL2: 176/176 including the live tests (B0: 163). The fuzz crate passes 15 tests (1 ignored by design) on both. `uefi-test` and `repro-test` PASSED. fmt, clippy (0 findings), docs, the `no_std`/UEFI builds and `cargo deny` pass on both; cargo-deny warns only about the known duplicate `bitflags` and `syn`. |

**Known limits:**
- CI has not run on GitHub yet. If the push token lacks the Workflows permission, GitHub rejects the workflow file.
- The console is not interactive: no raw mode, no Ctrl-C passthrough, and input only after the guest's first hvc0 write (the M6h port gate).
- If the terminal falls more than 1 MiB behind, output is dropped and counted.
- Fuzzing so far is three minutes per target, on Windows only. Exit dispatch is not fuzzed.
- Nothing here ran on the ThinkPad.

## Track W: Windows host (later)

| # | Milestone | Acceptance criteria | Depends on | Status |
|---|---|---|---|---|
| W1 | WHP backend probe | `aegis doctor` on Windows reports whether the WHP API is available (`WHvGetCapability`) | M1 | not started |
| W2 | WHP: first guest | The M2 test guest runs under WHP with the same assertions | W1, B3 | not started |

## Integration

| # | Item | Acceptance criteria | Depends on | Status |
|---|---|---|---|---|
| I1 | `pyxforge.toml` | Profiles: build workspace, run unit tests, run B0 nested QEMU (needs WSL), GDB attach to `-s -S`. Verified by opening the Aegis repo in PyxForge and running each profile. | M0, B0 | not started |
| I2 | PyxisOS hook | Documented `aegis doctor --json` schema that `pyxis` could consume. **No PyxisOS change.** | M1 | not started |

---

## Releases

| Version | Contents | Label |
|---|---|---|
| v0.1.0 | M0–M3 | *Experimental.* Hosted KVM on Linux, tested in WSL2 only, unless M5h has run. |
| v0.2.0 | B0–B3 | *Experimental.* Bare-metal VMX tested nested under KVM only. |
| v0.3.0 | M4h, M6h, B4–B5, I1 | Experimental |

Each release ships release notes listing what was tested where, and known limitations.

## Owner actions that unblock this roadmap

| Action | Unblocks |
|---|---|
| Revoke the pasted GitHub token; re-authenticate `gh` | Any push, CI |
| Add `LICENSE` (Apache-2.0 per ADR-0011) | Licensing of all code |
| ~~Add the WSL user to `kvm`~~: **done 2026-10-26** (`kvm:x:993:obsti`) | M2, M3, B0+ (now unblocked) |
| Approve installing Rust in WSL2 (rustup) | M0 in WSL2 |
| ~~Approve installing `cargo-deny`~~: **done** (0.20.2, M0) | M0 deny check |
| Approve installing `cargo-fuzz`; for fuzzing in WSL2, also nightly Rust and a C++ compiler (`clang`) | The standard cargo-fuzz workflow (docs/fuzzing.md). Fuzzing already works on Windows without these. |
| Push the local commits; if GitHub rejects `.github/workflows/ci.yml`, give the token the Workflows permission | CI's first run on GitHub, then README Phase 0 |
| Run `scripts/hardware-validation.sh` on the ThinkPad and bring back `results.md` | M5h |
| Decide on `ucs2` (MPL-2.0): keep `uefi-raw`, or allow `ucs2` alone and use `uefi` (licensing review §3.1) | Choice of UEFI crate for track B |
| Confirm ADR-0012 (canonical home) | README and PyxisOS docs consistency |

## Immediate next step

*Updated 2026-10-27.* M0–M3, M4h, M6h and B0 are done. The next steps are the owner actions above: push and the first CI run, the ThinkPad run (M5h), and the `ucs2` decision. The milestone tables give the order of the remaining development work.
