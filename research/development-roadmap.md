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
| M5h | PyxisOS Track A validation | M3 tests rerun on the ThinkPad X1 Carbon (i7-7600U). Results are recorded in `compatibility-matrix.md` as hardware-tested. | Physical hardware | M3; owner runs it on that machine | not started |

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
- virtio-console is output-only: there is no host input, and the receive queue stays empty. There is no `VIRTIO_CONSOLE_F_SIZE`, no multiport, and no `EVENT_IDX`.
- Device emulation runs on the vCPU thread, under a single bus lock. That's fine for a console, not a design for fast devices.
- Guest output is still printed after the run, not streamed.
- At most 8 vCPUs (the MADT builder's limit). There's no CPU hotplug, NUMA or topology leaves beyond the APIC ID. The vCPUs are not pinned.
- The summary shows the device status at stop, which is 0. The guest's last write to `Status` was a reset; since the queues only work after `DRIVER_OK`, that reset came after all output, on the way to reboot.
- The ACPI tables are checked by unit tests and by one guest kernel (Linux 6.18 via ACPICA). No other OS or firmware has consumed them, and no ACPI compiler (`iasl`) was run on them.
- No fuzzing (see the scope note).

## Track B: bare metal (VMX, UEFI)

| # | Milestone | Acceptance criteria | Environment | Depends on | Status |
|---|---|---|---|---|---|
| **B0** | Nested test harness | `cargo xtask uefi-test` builds `aegis-uefi.efi`, creates a FAT image, boots it in WSL2 `qemu-system-x86_64 -enable-kvm -cpu host -bios OVMF`, captures serial to a file, asserts a "hello" marker, and enforces a timeout. Failure output includes the serial log. | WSL2 QEMU/KVM | M0 | not started |
| **B1** | VMX capability + VMXON | In the L2 guest: checks CPUID.1:ECX.VMX and `IA32_FEATURE_CONTROL` (lock/enable bits); reads `IA32_VMX_BASIC` and the true-controls MSRs; applies the CR0/CR4 fixed bits; VMXON; VMXOFF; reports each step over serial. Clean failure paths are tested by forcing each check to fail (unit-tested decision logic over captured MSR values). | WSL2 nested | B0 | not started |
| **B2** | First guest | VMCS with deny-by-default MSR and I/O bitmaps; EPT built from a guest memory map (ADR-0006) and walked by a test to prove hypervisor pages are absent; a guest running a few instructions → `CPUID` exit (handled) → `HLT` exit (stop). VM-entry failures report the VM-instruction error. | WSL2 nested | B1 | not started |
| **B3** | Shared interfaces | `aegis-uefi` implements the ADR-0007 `Vm`/`VmExit` model; the same `aegis-core` dispatch tests run against both backends' exit streams | Any + nested | B2, M2 | not started |
| B4 | Serial device + I/O exits | The bare-metal guest writes to the 16550 via I/O exits, served by `aegis-devices` (shared with the hosted backend) | WSL2 nested | B3 | not started |
| B5 | Boot a guest kernel | Load a minimal 64-bit test kernel (later Linux) after `ExitBootServices`; long-mode guest state setup | WSL2 nested | B4 | not started |
| B6 | AMD SVM | Verify QEMU TCG SVM support first; then VMRUN + NPT with the B1/B2 criteria | QEMU TCG (to verify) | B2 | not started |
| B7 | Physical hardware | B2 criteria on the ThinkPad (i7-7600U) from a USB EFI boot | Physical hardware | B2; owner | not started |

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
| v0.3.0 | M4h, B4–B5, I1 | Experimental |

Each release ships release notes listing what was tested where, and known limitations.

## Owner actions that unblock this roadmap

| Action | Unblocks |
|---|---|
| Revoke the pasted GitHub token; re-authenticate `gh` | Any push, CI |
| Add `LICENSE` (Apache-2.0 per ADR-0011) | Licensing of all code |
| ~~Add the WSL user to `kvm`~~: **done 2026-10-26** (`kvm:x:993:obsti`) | M2, M3, B0+ (now unblocked) |
| Approve installing Rust in WSL2 (rustup) | M0 in WSL2 |
| Approve installing `cargo-deny` (and later `cargo-fuzz`) | M0 deny check, fuzzing |
| Confirm ADR-0012 (canonical home) | README and PyxisOS docs consistency |

## Immediate next step

**M0**, which can start now on Windows: workspace, `aegis-core`, and the `no_std` build for `x86_64-unknown-uefi`. The WSL2 parts of M0, and everything from M2 on, wait for the owner actions above.
