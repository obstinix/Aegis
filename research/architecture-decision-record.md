# Architecture Decision Records (Phase 3)

- **Date:** 2026-10-26.
- **Basis:** [`hypervisor-landscape.md`](hypervisor-landscape.md), [`repository-comparison.md`](repository-comparison.md), [`licensing-review.md`](licensing-review.md), [`compatibility-matrix.md`](compatibility-matrix.md), [`security-analysis.md`](security-analysis.md).
- **Status values:** `Accepted` (owner-directed or no owner input required) · `Proposed` (needs an owner decision) · `Superseded`.
- **Nothing here is implemented.** These records define what Phase 4 builds. Status in the README changes only when code and tests land.

| ADR | Title | Status |
|---|---|---|
| 0001 | Staged architecture: hosted KVM first, bare-metal core in parallel | **Accepted** (owner directed, 2026-10-26) |
| 0002 | Language and toolchain | Accepted |
| 0003 | Workspace layout and crate boundaries | Accepted |
| 0004 | Hosted backend: Linux KVM via rust-vmm | Accepted |
| 0005 | Bare-metal core: UEFI application, Intel VMX first | Accepted |
| 0006 | Memory management | Accepted |
| 0007 | VM lifecycle and exit dispatch | Accepted |
| 0008 | Devices and interrupts | Accepted |
| 0009 | Management interface: CLI only | Accepted |
| 0010 | Testing architecture and honest status reporting | Accepted |
| 0011 | Licensing: Apache-2.0 outbound | **Proposed**: needs the owner to add `LICENSE` |
| 0012 | Canonical home relative to `PyxisOS/native/aegis` | **Proposed**: owner decision |

---

## Goals and non-goals

**Goals (first release line, v0.x):**
1. Create, configure, run, observe and shut down a real VM: a test guest, then a Linux guest, on a hardware-virtualized host. Tested in automation.
2. A truthful capability report (`aegis doctor`) for every supported environment.
3. A from-scratch x86-64 VMX core that reaches VM entry and handles exits, in a nested test environment, behind the same interfaces as the hosted backend.
4. Security-conscious defaults from the first line of code (see `security-analysis.md` §3).

**Non-goals (v0.x):** GUI, web UI, daemon or network API; device passthrough; live migration; SMP in the bare-metal core; ARM64/RISC-V; Windows guests; production use or any isolation guarantee; Kubernetes or distributed orchestration.

---

## ADR-0001: Staged architecture

**Context.** The README targets a from-scratch VT-x/AMD-V hypervisor. Phase 2 found:
- No surveyed bare-metal project is a fit foundation.
- The dev workstation hides VMX (Hyper-V/VBS active).
- WSL2 provides KVM with nested VMX.
- PyxisOS Track A is Linux with KVM. Its Track B kernel cannot host a hypervisor yet.

**Decision.** Aegis has two backends behind one set of interfaces (ADR-0003, ADR-0007):
- **Stage 1, hosted:** a Linux KVM backend (ADR-0004). It delivers working VMs first.
- **Stage 2, bare metal:** a clean-room VMX core as a UEFI application (ADR-0005), developed in parallel and tested as an L2 guest under QEMU/KVM.
- **Later:** a WHP backend for Windows hosts, and an SVM path. Each lands only when it can be tested.

**Consequences.**
- The README sentence "It is not a wrapper around KVM, Xen, or Hyper-V" is no longer accurate for stage 1 and must be revised (done alongside this ADR).
- The bare-metal ambition is kept, with a defined test path.
- The interface design is constrained by both backends, which prevents a KVM-shaped API that the bare-metal core can't implement.

**Alternatives rejected.**
- *Bare-metal only:* no functional VM for many months, and no local raw-VMX test path.
- *Hosted only:* abandons the project's stated identity.
- *Fork an existing project:* see `repository-comparison.md` §5 Q4.

## ADR-0002: Language and toolchain

**Decision.**
- **Rust stable**, pinned via `rust-toolchain.toml` to `1.97.0` (the version observed on the workstation 2026-10-26). PyxisOS uses nightly; Aegis deliberately doesn't, so that `x86_64-unknown-uefi` (a stable tier-2 target) and Linux builds stay reproducible.
- `unsafe` is confined to backend crates and arch modules. Every `unsafe` block carries a `// SAFETY:` comment, and `#![deny(unsafe_op_in_unsafe_fn)]` is set crate-wide.
- **Assembly:** inline `asm!` / `global_asm!` for VMX instructions and entry stubs. **NASM** is used only for tiny test-guest images under `tests/guests/`, which matches the PyxisOS Track B toolchain. No C or C++ in Aegis.
- Lints: `cargo clippy -- -D warnings`, `cargo fmt --check`.

**Consequence.** The hosted backend builds and runs in Linux (WSL2 locally, Linux CI). The Windows host builds the UEFI core and the platform-independent crates.

## ADR-0003: Workspace layout and crate boundaries

```
Cargo.toml                 workspace
rust-toolchain.toml
crates/
  aegis-core/      #![no_std] + alloc. VmConfig model, lifecycle state machine, VmExit enum,
                   device-bus traits, GuestMemory region map + validation. No OS, no unsafe.
  aegis-probe/     Capability detection. CPUID decoding is no_std (default-features = false).
                   Feature "std" (default) adds the Linux /proc and /sys parsers (they build on
                   every OS, so fixtures test them anywhere), the doctor report, and serde.
                   The live /dev/kvm probe and kvm-ioctls are compiled only on Linux.
                   (Revised in M1 from a separate "linux" feature; see the devlog.)
  aegis-kvm/       Hosted backend (Linux only): rust-vmm kvm-ioctls, vm-memory, linux-loader.
  aegis-uefi/      Bare-metal VMX core, target x86_64-unknown-uefi (crate `uefi`).
  aegis-devices/   Backend-neutral device models (16550 UART first).
  aegis-cli/       Binary `aegis`: doctor, run, version.
xtask/             Build/test orchestration: assemble guests, build EFI, run QEMU tests.
tests/guests/      NASM sources for test guests (sources only; built artifacts are ignored).
research/  docs/
pyxforge.toml      PyxForge build/QEMU/GDB profiles (ADR-0009 integration point)
```

**Rules.**
- `aegis-core` depends on nothing platform-specific. Both backends depend on `aegis-core`, never on each other.
- The CLI selects a backend at runtime, based on `aegis-probe` results and the config.

## ADR-0004: Hosted backend: Linux KVM via rust-vmm

**Decision.**
- Use the published rust-vmm crates (`kvm-ioctls`, `kvm-bindings`, `vm-memory`, `linux-loader`; later `virtio-queue`, `seccompiler`, `event-manager`) as versioned dependencies, with no fork.
- The initial host platform is **Linux x86-64 with KVM**: WSL2 locally, PyxisOS Track A later.
- One process per VM and one thread per vCPU.

**Consequences.**
- The KVM in-kernel code is part of the TCB.
- Aegis owns the device models and the VMM process hardening.
- WHP support (Windows host) is a separate later backend, modeled on Hyperlight's approach and added only once it can be tested.
- **Blocker (resolved 2026-10-26):** the WSL user had to be added to the `kvm` group before any test could run. The owner did this, and `aegis doctor` confirms `/dev/kvm` opens.
- **M3 note: `linux-loader` not used.** `linux-loader` 0.14 leaves `ByteValued` for `boot_params` to the VMM, which means an `unsafe impl` (Firecracker does this). Instead, `aegis-kvm::boot` follows `Documentation/arch/x86/boot.rst` directly: it copies the setup-header *bytes* from the bzImage into the zero page and patches documented offsets. That needs no `unsafe` and no extra dependency. The cost is owning ~200 lines of protocol code, covered by unit tests on synthetic images and the live boot test.

## ADR-0005: Bare-metal core: UEFI application, Intel VMX first

**Decision.**
- `aegis-uefi` is a UEFI application. Early milestones run it **with boot services still active, on the BSP only**:
  - check CPUID and `IA32_FEATURE_CONTROL`;
  - allocate VMXON/VMCS/EPT pages with `AllocatePages`;
  - VMXON;
  - set up a guest with its own memory (not a blue-pill of the firmware);
  - VMLAUNCH;
  - handle exits;
  - VMXOFF and return.
- `ExitBootServices`, multiple CPUs, and booting Linux are later milestones.
- **Intel VMX first.** It's the only hardware path testable here, through nested KVM. SVM comes after, tested under QEMU TCG once that is verified.
- **No dependency on APICv, posted interrupts, VMCS shadowing or PML.** None of them are exposed to L1 in WSL2.
- Firmware: UEFI only. Legacy BIOS is unsupported.

**Reference use (clean-room).** Written against Intel SDM Vol. 3C. barevisor, RVM1.5 `libvmm` and ymir (all MIT) may be consulted for structure; consultation is recorded in the devlog. No code is copied unless `licensing-review.md` §5 is updated first.

## ADR-0006: Memory management

**Decision.**
- **Guest memory map** (in `aegis-core`): an ordered, non-overlapping list of GPA regions with explicit permissions. Validated at construction: no overlap, page-aligned, total ≤ the configured cap, checked arithmetic.
- **Hosted:** `vm-memory` `GuestMemoryMmap` backed by anonymous private mappings, registered with `KVM_SET_USER_MEMORY_REGION`. All guest accesses by the VMM go through `vm-memory` accessors, which do bounds checks. Data a decision depends on is copied once, then validated.
- **Bare metal:**
  - EPT is 4-level, with 4 KiB and 2 MiB pages, built *only* from the guest memory map. Hypervisor-owned pages are never mapped.
  - A post-build EPT walk asserts this (test).
  - Every permission reduction or unmap is followed by INVEPT.
  - Guest memory is allocated host-physical, so no identity mapping of host RAM is used.
  - EPT memory type comes from the MTRRs when needed: UC for MMIO ranges, WB for RAM.

## ADR-0007: VM lifecycle and exit dispatch

**Decision.**
- **Lifecycle** (state machine in `aegis-core`, unit-tested exhaustively):
  `Created → Configured → Running ⇄ Paused → Stopped → Destroyed`, with `Failed` reachable from any active state. Invalid transitions return errors and never panic.
- **Backend trait** (sketch):
  ```rust
  trait Backend { type Vm: Vm; fn probe() -> Capabilities; fn create_vm(&self, cfg: &VmConfig) -> Result<Self::Vm>; }
  trait Vm { fn run_vcpu(&mut self, id: VcpuId) -> Result<VmExit>; fn pause(&mut self); fn stop(&mut self) -> Result<()>; }
  enum VmExit { Io { port: u16, size: u8, write: Option<u32> }, Mmio { gpa: u64, size: u8, write: Option<u64> },
                Hlt, Shutdown, Cpuid { leaf: u32, subleaf: u32 }, Unhandled { raw_reason: u64 } }
  ```
- **Dispatch:** `aegis-core` routes `Io`/`Mmio` to a device bus. `Unhandled` stops *that VM* with diagnostics (never silently continued).
- **Accounting:** per-exit-reason counters are kept for `aegis` output and for exit-storm detection.

The final signatures are fixed in M0 code review, not here.

**As implemented (M2, backend-neutral part).** The sketch above was revised:
- **Port I/O inside `run`.** `Vcpu::run(&mut self, io: &mut dyn PortIo) -> Result<VmExit, Self::Error>` serves port I/O itself, before the guest resumes. KVM (and VMX) need a port read's result before the next VM entry, and `kvm-ioctls` exposes the I/O data only for the duration of the exit. The exit is still returned, so every exit is traced.
- **Dispatch loop.** `run_vcpu(vcpu, io, lifecycle, limits)` loops over exits and moves the lifecycle to `Stopped` (HLT, shutdown) or `Failed` (unhandled MMIO, unhandled exit, exit limit, backend error). It returns a `RunOutcome`: the stop reason, an exit count, and a bounded trace.
- **Port bus.** `PortBus` borrows its `PortDevice`s. Unassigned ports read as all ones and drop writes; both are logged, and the guest continues.
- **Not built yet.** No `VmConfig`, `Backend` or `pause` exists yet. They arrive with the KVM backend, so their shape can be checked against a real implementation.

**As implemented (M4h, SMP and MMIO).**
- **MMIO inside `run`, like port I/O.** `Vcpu::run` now takes `&mut dyn GuestIo` (`PortIo` + `MmioIo`). An access a device claims returns the resumable `VmExit::MmioDevice`; an unclaimed one returns `VmExit::Mmio`, which still stops the VM (it is no longer resumable).
- **One bus for both.** `PortBus` became `DeviceBus`, with port and MMIO windows. An MMIO access must fall entirely inside one window.
- **Stopping a vCPU from outside.** `run_vcpu_until(vcpu, io, limits, stop)` checks a `StopFlag` before each entry. A backend error while the flag is set (the kick's `EINTR`) is `StopReason::Requested`, not a failure. `vm_stop_event` combines the per-vCPU reasons: `Requested` is ignored, and the VM stops normally only if every other reason is normal.
- **Threads.** `aegis-kvm::smp` runs one scoped thread per vCPU over a mutex-protected `DeviceBus`. The first vCPU that stops for its own reason sets the flag, and the coordinator re-sends `SIGRTMIN` (raw `tgkill`) to the others every 10 ms until they finish. These are the backend's second and third `unsafe` blocks (`gettid`, `tgkill`), each with a SAFETY comment.
- **KVM detail:** an application processor's `KVM_RUN` returns `EAGAIN` after INIT and SIPI change its state. The backend runs it again. That's not an error.

## ADR-0008: Devices and interrupts

**Decision.**
- **v0.x device set:** a 16550 UART at `0x3F8`. Output first; input later. Nothing else by default.
- **Linux guest (hosted):** KVM in-kernel irqchip and PIT (`KVM_CREATE_IRQCHIP`, `KVM_CREATE_PIT2`), so Aegis doesn't emulate PIC/IOAPIC/PIT itself in stage 1. virtio-blk/net over the MMIO transport (`virtio-queue`) come in a later milestone.
- **Bare metal:**
  - Guests initially run with interrupts masked.
  - External-interrupt exiting is enabled, so host interrupts are never delivered to the guest.
  - Event injection is validated (vector, type, error-code consistency) before VM entry.
  - LAPIC emulation is a later milestone.
- **Passthrough:** unsupported until IOMMU enforcement exists (`security-analysis.md` §3.5).

**As implemented (M4h).**
- **Device:** virtio-console, not virtio-blk. The test kernel builds virtio-blk as a module; see the roadmap's M4h scope note.
- **Transport:** virtio-mmio version 2 at GPA `0xD000_0000` (4 KiB window), virtio 1.x only. `VIRTIO_F_VERSION_1` must be accepted before `FEATURES_OK`.
- **Discovery through ACPI**, since the test kernel has neither MP-table parsing nor virtio-mmio command-line devices. `aegis-kvm::acpi` builds the tables with `acpi_tables` 0.2.1 at `0xE_0000` (inside the legacy BIOS area the kernel also scans) and passes the RSDP in `boot_params.acpi_rsdp_addr` (boot protocol ≥ 2.14):
  - RSDP → XSDT;
  - FADT, hardware-reduced, pointing at the DSDT;
  - MADT, with one enabled LAPIC per vCPU and the IOAPIC at GSI base 0;
  - DSDT, with one `LNRO0005` device per virtio-mmio window: `_HID`, `_UID` and a `_CRS` with Memory32Fixed plus a level-triggered, active-high, exclusive interrupt. This is the same shape as QEMU's microvm.

  The tables are built only for SMP (`vcpus > 1`) or virtio. Otherwise the guest sees the unchanged M3 machine, with no ACPI.
- **Interrupt:** GSI 5, level-triggered through the in-kernel IOAPIC with `KVM_IRQ_LINE`. The line follows `InterruptStatus`: raised for used buffers (unless the driver set `VRING_AVAIL_F_NO_INTERRUPT`) and for `DEVICE_NEEDS_RESET`, and lowered by `InterruptACK`.
- **Queue code is Aegis's own, not `virtio-queue`.** `aegis-devices` is `no_std` and backend-neutral (ADR-0003), and its devices reach guest memory only through `aegis-core`'s `GuestMemoryAccess` trait. `virtio-queue` is built on `vm-memory`'s `GuestMemory`, which would tie the device crate to the hosted backend. The split-queue subset used is small: no indirect descriptors and no `EVENT_IDX`.

**Decision.**
- **Binary `aegis` commands:**
  - `aegis doctor [--json]`
  - `aegis run --config <vm.toml>`
  - `aegis version`
- **Config:** TOML parsed with `serde` (`deny_unknown_fields`), size-limited, with paths canonicalized and restricted.
- **Output:** human-readable by default. `--json` emits a stable, versioned schema for PyxisOS and PyxForge to consume.
- **Not in v0.x:** no daemon, socket, REST/gRPC or GUI.
- **Integration points:**
  - Aegis ships `pyxforge.toml` profiles (build, test, QEMU L2 run, GDB attach). This uses PyxForge's existing `internal/build`, `internal/qemu` and `internal/gdb`, with no PyxForge code change.
  - PyxisOS `pyxis doctor` may call `aegis doctor --json` later. **No PyxisOS change is part of this plan.**

## ADR-0010: Testing architecture and honest status reporting

**Decision.**

| Layer | Runs where | Content |
|---|---|---|
| Unit | Any host (Windows or Linux), CI | `aegis-core`, `aegis-devices`, `aegis-probe` with captured CPUID/sysfs fixtures (including this workstation's WSL2 values) |
| Mock backend | Any host, CI | Lifecycle and dispatch driven by scripted `VmExit` sequences |
| KVM integration | Linux with an openable `/dev/kvm` | Test guests from `tests/guests/`; assert exit sequences, serial output, teardown |
| Nested bare-metal | WSL2: QEMU/KVM + OVMF, `-cpu host` | `xtask` builds the EFI image, boots it, captures serial to a file, asserts markers, enforces a timeout |
| Fuzz | Linux, local first | Exit dispatch, device registers, config parser |

- **Skips are reported as skips.** A KVM test that cannot open `/dev/kvm` prints the reason and is counted as skipped. It never passes silently. CI summaries list skipped suites.
- **CI** (GitHub Actions, once the owner enables pushes): fmt, clippy, unit, mock, `cargo deny`. KVM tests on GitHub-hosted Linux runners need `/dev/kvm` access; ⚠️ this is believed possible but **not verified**. Until it is, KVM and nested tests are local-only and labeled that way.
- **Tests are never deleted or weakened to make CI pass** (directive §11).

## ADR-0011: Licensing: Apache-2.0 outbound (Proposed)

**Decision (proposed):**
- Aegis is licensed **Apache-2.0**, matching the README badge, PyxisOS and PyxForge.
- `cargo deny` license allowlist: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib. Anything else fails the build.
- Clean-room rule per `licensing-review.md` §4.

**Owner action:** add the `LICENSE` file. Until then, Phase 4 code is not considered licensed to anyone else.

## ADR-0012: Canonical home relative to `PyxisOS/native/aegis` (Proposed)

**Context.**
- `PyxisOS/native/aegis` is a std-only capability/sandbox model with 4 tests. Its VT-x detection is a stub.
- `obstinix/Aegis` is the repository this directive targets.

**Decision (proposed):**
- **`obstinix/Aegis` is the canonical hypervisor project.**
- `PyxisOS/native/aegis` remains PyxisOS's own capability model, unchanged by this plan.
- Later, the owner may replace its stub detection by depending on `aegis-probe`, or rename it to avoid confusion.

**Owner action:** confirm, or choose otherwise.

---

## Expected limitations (v0.x, stated in advance)

- Hosted path only on Linux/KVM. No Windows host backend.
- Bare-metal core: Intel only, BSP only, nested-emulator tested only. **Not tested on physical hardware.**
- No passthrough, no SMP bare-metal guests, no migration, no snapshot.
- No isolation guarantees. Research software. The README warning stays until an external security review.
