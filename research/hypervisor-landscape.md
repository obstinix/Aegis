# Hypervisor Landscape — Phase 1 Discovery

- **Status:** Phase 1 deliverable. Findings, not decisions. The architecture decision belongs to Phase 3 (`architecture-decision-record.md`, not yet written).
- **Observed:** 2026-10-26. All star counts, licenses, and activity dates are as reported by the GitHub API on that date.
- **Method:** GitHub REST API metadata for all 570 `topic:hypervisor` repos; shallow clones plus source reading for 17 shortlisted repos; local inspection of the PyxisOS and PyxForge zip extracts; local host capability checks. Nothing from a third-party repo was built or executed.

---

## 1. What was and was not inspected

| Scope | Coverage | Notes |
|---|---|---|
| GitHub `hypervisor` topic | **570 / 570 repos: metadata** | API reported `incomplete_results=false`. Full table: [`data/hypervisor-topic-2026-10-26.md`](data/hypervisor-topic-2026-10-26.md). |
| Topic repos: classification | Heuristic, from description and topics | Keyword triage only. Not verified per repo. |
| Topic repos: source read | 14 repos | Listed in §4. Every other topic repo was triaged on metadata alone. |
| §3B required repos | 3 / 3, source or README read | §5 |
| Non-topic upstream projects | Metadata for 9; source read for rust-vmm | Firecracker, Cloud Hypervisor, crosvm, ACRN, Xen and bhyve were **not** source-read this session. Statements about them come from their public documentation and are marked as such in §6. |
| PyxisOS | Local zip extract, read key files | Not a Git checkout. See §2. |
| PyxForge | Local zip extract, read layout and README | Not a Git checkout. See §2. |

Grep evidence in §4 counts **files that mention** a mechanism (e.g. `vmlaunch`, `ept`). It shows a mechanism is present in the code, not that it is complete or correct.

---

## 2. Workspace and repository state

| Repo | Location | Git state | Notes |
|---|---|---|---|
| Aegis | `C:\aegis environment\Aegis` | Cloned 2026-10-26 from `https://github.com/obstinix/Aegis.git`. Branch `main`, HEAD `a424ee6`, clean. 3 commits, all 2026-10-18. | Only file: `README.md`. |
| PyxisOS | `C:\aegis environment\PyxisOS-main (1)\PyxisOS-main` | **Not a Git repo** (zip extract). Upstream `obstinix/PyxisOS`, last push 2026-10-07. | Read-only for this work. |
| PyxForge | `C:\aegis environment\PyxForge-main (2)\PyxForge-main` | **Not a Git repo** (zip extract). Upstream `obstinix/PyxForge`, last push 2026-10-25. | Read-only for this work. |

Git identity found (global): `user.name=obstinix`, `user.email=obstinix@gmail.com`. The existing Aegis commits were authored as `Piyush Pandey <139326479+obstinix@users.noreply.github.com>`. **Whether `obstinix@gmail.com` is linked to the `obstinix` account was not verified** (it needs an authenticated API call). See §9.

`gh` CLI 2.102.0 is configured for `obstinix`, but its stored keyring login is **failing**. Authenticated operations (push, private API calls) are blocked until the owner re-authenticates.

### 2.1 Aegis: actual implementation status

- **Code:** none. **Build system:** none. **Tests:** none. **License file:** none, although the README advertises Apache-2.0 via a badge.
- The README is accurate about this: "Aegis has no working code yet." It sets the target as a from-scratch x86-64 VT-x/AMD-V hypervisor in Rust + NASM, part of PyxisOS Track B next to Lunar Core, explicitly "not a wrapper around KVM, Xen, or Hyper-V."

### 2.2 PyxisOS: what is real

- **Track A (verified on hardware, per its own status doc):** Linux 6.x on a ThinkPad X1 Carbon 5th Gen (i7-7600U, Kaby Lake), UEFI via systemd-boot, KDE Plasma 6, a POSIX shell `pyxis` CLI. A Kaby Lake i7 provides VT-x with EPT and VT-d. That is inferred from the CPU model, not verified on the machine.
- **Track B (in development or experimental):** a freestanding C/ASM kernel (Multiboot 1 → long mode, GDT/IDT/TSS, 8259 PIC, PMM/VMM/heap, UART/VGA/PIT/PS2 drivers) and `native/lunar-core`, a `#![no_std]` Rust crate whose `_start` sets up serial and GDT, prints, and halts. `BootInfo::empty()` is used, so it does not yet parse a real boot handoff. Boot is QEMU `-kernel` (Multiboot), not UEFI.
- **`native/aegis/` already exists inside PyxisOS.** It is a hosted (std, user-mode) Rust library with 383 lines: capability tokens, rights masks, a sandbox state machine with memory accounting, a `SoftwareIsolation` checker, and 4 unit tests. `HardwareVirtualizationInterface::is_virtualization_supported()` **always returns `false`**: the CPUID check is a comment, not code. The crate's README says it is "Not started — and shouldn't be, until `native/lunar-core/` exists," and that "Track A's near-term sandboxing need is met by KVM/Firecracker."
- **Implication:** the standalone `obstinix/Aegis` repo and `PyxisOS/native/aegis` overlap in name and intent. The owner needs to decide which one is canonical (§9).

### 2.3 PyxForge: what is real

- Go 1.27.1 + Fyne v2.8.1 desktop app with embedded Neovim (go-client). It has `internal/qemu`, `internal/gdb`, `internal/build`, and `pyxforge.toml` build profiles. Its README says QEMU/GDB workflows are implemented and tested on Windows and Linux, and that agents are not implemented.
- **Implication:** the natural integration is for Aegis to ship a `pyxforge.toml` with build, QEMU and GDB profiles. That needs **no PyxForge code change** and keeps Aegis's core free of UI dependencies. To be evaluated in Phase 2.

---

## 3. GitHub `hypervisor` topic: overview

570 repos. Heuristic split: 176 management/tooling, 123 x86 or unspecified hypervisors, 59 offensive/detection/RE, 58 hosted VMMs (KVM/WHP/HVF), 53 non-x86 hypervisors, 39 education/resources, 38 emulators, 24 introspection/debug/fuzz. 44 are archived; 95 have ≥100 stars; 224 have ≥10.

**The highest-starred repos are mostly *not* hypervisors you could build on.** Of the top 12 by stars:

| Stars | Repo | What it actually is |
|---|---|---|
| 5291 | zhkl0228/unidbg | Android/iOS native-library **emulator** (Java). Not a hypervisor. |
| 4758 | hyperlight-dev/hyperlight | **Hosted** micro-VM manager (Rust, Apache-2.0) embedded in apps, over KVM / MSHV / WHP. Real and active. |
| 4153 | HyperDbg/HyperDbg | VT-x-based **debugger** (C, GPL-3.0). |
| 2674 | SinaKarvandi/Hypervisor-From-Scratch | **Tutorial** source (MIT). |
| 2203 | TrungNguyen1909/qemu-t8030 | iPhone **emulation** on QEMU. Archived. |
| 2014 | ZSvirt/zsvirt | IaaS **management** engine (Java). |
| 2004 | hyperhq/hyperd | Container daemon. Archived 2018. |
| 1986 | Wenzel/awesome-virtualization | **Resource list.** |
| 1975 | siemens/jailhouse | **Real** Linux-based partitioning hypervisor (C, GPL-2.0). x86 + ARM. |
| 1760 | tandasat/HyperPlatform | Windows VT-x VM-exit filtering platform (C++, MIT). Archived. |
| 1667 | Air14/HyperHide | Anti-anti-debug plugin. |
| 1568 | nextcloud/vm | Nextcloud VM appliance scripts. |

Stars measure attention, not suitability. Firecracker (37.6k stars), Cloud Hypervisor (6.4k), crosvm, ACRN and Xen are **not tagged** with the topic, so a topic-only survey would miss the most mature open-source VMMs.

---

## 4. Source-read shortlist (x86-64 relevant)

| Repo | Stars | License (verified from file) | Lang / LoC* | Type | Mechanisms present in code (file hits) | Boot / host | Notes |
|---|---|---|---|---|---|---|---|
| [tandasat/barevisor](https://github.com/tandasat/barevisor) | 385 | MIT | Rust / 5.7k | Educational, bare minimum | VMX + SVM, EPT + NPT, MTRR | UEFI driver **or** Windows driver; blue-pill style (virtualizes the running OS) | Clean Intel/AMD abstraction in stable Rust. Tests in Bochs/VMware via `xtask`. Last push 2026-08-02. |
| [tandasat/Hypervisor-101-in-Rust](https://github.com/tandasat/Hypervisor-101-in-Rust) | 1213 | MIT | Rust / 4.9k | Course material | VMX + SVM, EPT + NPT | UEFI app | A snapshot **fuzzer** built as teaching material. Runs guests from snapshots; not a general VMM. |
| [tandasat/MiniVisorPkg](https://github.com/tandasat/MiniVisorPkg) | 762 | MIT | C / 32.6k | Research | VMX, EPT | UEFI (EDK2) and Windows | Boots an OS under the hypervisor. Intel only. |
| [memN0ps/illusion-rs](https://github.com/memN0ps/illusion-rs) | 413 | MIT | Rust / 11.7k | Research (hooking) | VMX, EPT, INVEPT/INVVPID, MTRR | UEFI blue-pill under Windows | Focused on stealth EPT hooks. **Archived.** Intel only. |
| [smallkirby/ymir](https://github.com/smallkirby/ymir) | 69 | MIT | Zig / 9.6k | Educational, type-1 | VMX, EPT | UEFI loader (`surtr`) → hypervisor → **boots Linux 6.9 with initramfs** | README: no SMP, no APIC. The best match to "bare-metal hypervisor boots its own guest" among small projects. Zig, not Rust. |
| [rcore-os/RVM1.5](https://github.com/rcore-os/RVM1.5) | 73 | MIT | Rust / 7.4k | Research, "type-1.5" | VMX + SVM, EPT + NPT, AMD IOMMU file | Loaded from a running Linux (Jailhouse-style), then demotes Linux into a cell | `libvmm` crate has typed VMCS/VMCB definitions. Last push 2024-05. |
| [Zero-Tang/NoirVisor](https://github.com/Zero-Tang/NoirVisor) | 658 | Apache-2.0 / MIT (dual files present) | C + Rust / 32.7k | Research | VMX + SVM, EPT + NPT, IOMMU/DMA files, nested-virt | UEFI and Windows driver | Active (2026-10-25). Large C core, Rust in newer crates. |
| [Bareflank/hypervisor](https://github.com/Bareflank/hypervisor) | 1518 | MIT (`LICENSE.md`) | C++ / 202k | SDK | VMX + SVM, EPT | Windows, Linux, UEFI loaders | Large. Last push 2024-08. |
| [siemens/jailhouse](https://github.com/siemens/jailhouse) | 1975 | **GPL-2.0** (with a hypercall-use exception) | C / 63.8k | Production partitioning | VMX + SVM, EPT + NPT, VT-d/AMD-IOMMU, x2APIC/posted interrupts | Loaded from Linux; static partitioning, no scheduling | Strong reference for IOMMU and interrupt isolation. **GPL-2.0 makes code reuse into an Apache-2.0 Aegis infeasible.** Last push 2024-05. |
| [bao-project/bao-hypervisor](https://github.com/bao-project/bao-hypervisor) | 585 | Apache-2.0 | C / 38.3k | Production static partitioning | ARM + RISC-V stage-2; SMMU/IOMMU | Bare metal | **No x86 support in source.** Reference only, for partitioning design. |
| [hyperlight-dev/hyperlight](https://github.com/hyperlight-dev/hyperlight) | 4758 | Apache-2.0 (+ NOTICE) | Rust / 108k | Production hosted micro-VM | Backends: KVM, MSHV, WHP | Linux and Windows hosts | Shows **one Rust codebase targeting KVM and Windows WHP**. That is directly relevant to a hosted Aegis track that runs on this Windows workstation and on PyxisOS Track A Linux. |
| [hermit-os/uhyve](https://github.com/hermit-os/uhyve) | 296 | Apache-2.0 / MIT | Rust / 15.9k | Specialized hosted VMM | KVM (+ macOS HVF) | Linux / macOS hosts | Runs Hermit unikernels only. |
| [bobuhiro11/gokvm](https://github.com/bobuhiro11/gokvm) | 267 | MIT | Go / 11.6k | Educational hosted VMM | KVM ioctls | Linux | Boots Linux. Small, readable. |
| [rust-vmm/rust-vmm](https://github.com/rust-vmm/rust-vmm) | 148 (monorepo; was split repos) | Per-crate: Apache-2.0, BSD-3, or dual (e.g. `kvm-ioctls` Apache-2.0 OR MIT) | Rust / 135k | VMM building blocks | `kvm-ioctls`, `kvm-bindings`, `vm-memory`, `linux-loader`, `vm-virtio`, `seccompiler`, `event-manager`, `acpi_tables` | Linux/KVM | **The shared foundation of Firecracker and Cloud Hypervisor.** Licensed for dependency use from Apache-2.0 code. |
| SinaKarvandi/Hypervisor-From-Scratch | 2674 | MIT | (tutorial source) | Tutorial | VMX, EPT | Windows driver | Learning reference. |

\* LoC = raw line count of `.rs/.c/.h/.cpp/.hpp/.S/.asm/.zig/.go` files, comments included.

---

## 5. §3B required repositories

### 5.1 mooofin/hypv64: **architectural reference for RISC-V only. Not a viable x86-64 foundation.**
- **It is a RISC-V 64 hypervisor, not x86-64.** Its README title is "riscv-hypervisor": "A RISC-V 64 hypervisor in Rust, built for QEMU `virt`."
- 803 lines of Rust. Implements: boot, an allocator, G-stage translation via `hgatp`, a Linux image loader with a generated DTB, launching the guest in VS-mode. Its README states: no device emulation, no interrupt forwarding, a single CPU, and guest traps "only panic."
- **No license file.** GitHub reports none, and none exists in the tree. Under default copyright, **its code cannot be reused.** Ideas only.
- The repo commits a 97 MB `linux/vmlinux` and a 5.4 MB `linux/Image` (Linux binaries) with no accompanying license or source notice.
- It shares no CPU-virtualization code paths with VT-x/SVM (`hgatp`/`hstatus` vs VMCS/VMCB). Its value to Aegis is limited to the general shape of a small Rust hypervisor (trap → vCPU → guest page table).

### 5.2 shogunlab/awesome-hyper-v-exploitation: **defensive-security / Hyper-V-architecture reference.**
- A curated link list (GPL-3.0 applies to the list text), with sections for conference talks, blog posts, references and resources, and research tools. It contains no implementation.
- Useful for §9 security work: real VM-escape classes in a production hypervisor (VMBus, synthetic devices, hypercall handlers, nested virtualization), and fuzzing approaches to emulate defensively. Linked material is read for understanding. No exploit code is to be run.

### 5.3 JonathanSalwan/VMProtect-devirtualization: **unrelated to Aegis's core goals.**
- Experimental research on **devirtualizing VMProtect 3.x-protected pure functions** with dynamic symbolic execution and LLVM. "Virtualization" here means *software obfuscation through a custom bytecode VM*. It is **not** hardware-assisted CPU virtualization.
- No license detected by GitHub; repo is notes plus samples (README read only). At most a tangential testing reference for symbolic-execution tooling. No relevance to VMX/SVM, EPT, or VM lifecycle.

---

## 6. Beyond the topic: upstream and established projects

**Not source-read this session.** These summaries come from the projects' public documentation and established knowledge. They must be re-verified against primary docs before any claim in the ADR depends on them. Metadata (stars, license per API, last push) was observed 2026-10-26.

| Project | Stars / license per API | Model | Relevance to Aegis |
|---|---|---|---|
| **Linux KVM** | (in-kernel) GPL-2.0 | Kernel module turning Linux into a hypervisor; userspace VMM through `/dev/kvm` ioctls | Available **now** in WSL2 on this workstation and on PyxisOS Track A. The most practical way to reach a working VM quickly. Using KVM's public ioctl API from userspace does not make Aegis a derivative of KVM. |
| **Firecracker** | 37,614 / Apache-2.0 | Minimal Rust VMM on KVM; virtio-net/block/vsock; jailer + seccomp | Security-minimal device-model reference: tiny attack surface, process jailing. |
| **Cloud Hypervisor** | 6,431 / API: not detected (project states Apache-2.0 AND BSD-3-Clause; **unverified this session**) | Rust VMM on KVM **and MSHV** | Reference for multi-backend design. |
| **crosvm** | 1,349 / BSD-3-Clause | Rust VMM (ChromeOS), sandboxed per-device processes | Reference for privilege-separated device emulation. |
| **rust-vmm** | 148 / per-crate permissive | Crates | **Candidate dependency** for a hosted backend (§4). |
| **Xen** | mirror 858 / GPL-2.0 (mixed) | Type-1 with dom0 | Architectural reference (PV, grant tables, XSM). No code reuse. |
| **ACRN** | 1,294 / BSD-3-Clause | Type-1 for IoT/automotive, x86, service VM | Permissive x86 type-1 reference. Last push 2026-04. |
| **bhyve** (FreeBSD) | in freebsd-src / BSD-2 | Type-2 on FreeBSD, VT-x/SVM, EPT/NPT | Permissive reference for a VMX/SVM kernel module plus userspace device model. |
| **Hyper-V / TLFS** | (closed source; public spec) | Microkernel type-1, root partition | Hypervisor Top-Level Functional Specification: reference for hypercall ABI and enlightenments. |
| **Windows Hypervisor Platform (WHP)** | Windows API | Userspace VM API over Hyper-V | The only way to run guests with hardware acceleration on this workstation natively while Hyper-V/VBS is active. Hyperlight uses it. |

---

## 7. Host and test environment (verified 2026-10-26)

| Check | Result |
|---|---|
| Workstation CPU | Intel Core Ultra 9 275HX (GenuineIntel) |
| Windows `HypervisorPresent` | **True.** Hyper-V/VBS is active, so Windows hides VMX/SLAT from the root partition (WMI reports `False` for both). |
| Raw VT-x on this host (bare-metal Aegis) | **Not available** without disabling Hyper-V/VBS, a security setting change the owner must make. Not recommended. |
| WSL2 | Ubuntu, WSL2, kernel `6.18.33.2-microsoft-standard-WSL2`. `/dev/kvm` **present** (`root:kvm`, mode 660). VMX flags visible in `/proc/cpuinfo`. `qemu-system-x86_64` installed. `cargo` **not** installed in WSL. |
| Windows toolchain | rustc/cargo/rustup, nasm, `qemu-system-x86_64`, gdb, go, clang all on PATH (versions not yet recorded). |

**Consequences (to be confirmed in Phase 2):**
1. A **hosted KVM** backend can be built and tested today inside WSL2. Whether the WSL user is in the `kvm` group still needs checking.
2. A **bare-metal VT-x prototype** can be exercised as a guest of QEMU/KVM in WSL2 with nested VMX (`-cpu host`). This depends on nested virtualization working through Hyper-V → WSL2 → KVM → guest, which was **not yet tested**. QEMU's TCG (no-KVM) mode on Windows does not emulate VMX, so bare-metal VMX testing cannot use plain Windows QEMU.
3. Physical-hardware validation (ThinkPad i7-7600U) is a separate, later step. Emulator success will not be reported as hardware support.

---

## 8. Preliminary observations (inputs to Phase 2, not decisions)

1. **No surveyed project should be forked as Aegis.** The permissive x86 bare-metal projects are educational or research-scale (barevisor, ymir, Hypervisor-101, MiniVisorPkg, RVM1.5). The production ones are GPL (Jailhouse, Xen, KVM) or not x86 (Bao).
2. **The strongest architectural references for a from-scratch x86-64 Rust core** appear to be barevisor (Intel/AMD abstraction in stable Rust), RVM1.5 (`libvmm` typed VMCS/VMCB) and ymir (a UEFI loader that boots Linux as a guest). All three are MIT, so ideas, and with attribution code, may be reused. The Phase 2 licensing review must confirm this per file.
3. **The strongest foundation for a *working* VM soon** appears to be rust-vmm crates on KVM, with Hyperlight as the reference for adding WHP (Windows) later.
4. A **staged** path (Option C) looks consistent with both the evidence and the existing PyxisOS docs: a hosted KVM backend first for real VM lifecycle, tests, and the CLI; a bare-metal VMX core developed in parallel and tested nested under KVM; both behind a shared vCPU/memory/exit-handler interface. Scoring in Phase 2 must either back this or overturn it.

---

## 9. Blockers and owner decisions needed

| # | Item | Why it matters |
|---|---|---|
| 1 | **A GitHub token was pasted into chat on 2026-10-26.** It has not been used or stored. The owner should revoke it and re-authenticate `gh` outside the chat. | `gh` keyring login is currently failing. Push is blocked until fixed. |
| 2 | Confirm the commit email: `obstinix@gmail.com` (current global config) vs `139326479+obstinix@users.noreply.github.com` (used on existing commits). | §12: commits must map to the `obstinix` account. |
| 3 | Add a `LICENSE` file to Aegis. The README claims Apache-2.0, but there is no license file. | Without it the repo is not licensed in practice, and outbound license compatibility can't be assessed. |
| 4 | Canonical home: standalone `obstinix/Aegis` vs `PyxisOS/native/aegis`. | Avoids two diverging "Aegis" crates. A suggested option: Aegis is canonical, and PyxisOS later depends on it. |
| 5 | Accept a staged scope that includes a KVM-hosted backend, given the README's "not a wrapper around KVM" statement. | Changes the project's stated identity, so it's the owner's call. Phase 3. |
| 6 | Push gate: no push until Phase 4 has verified work and the owner approves. | §0 |

## 10. Open questions for Phase 2

- Per-file license audit of barevisor, RVM1.5 and ymir before borrowing anything beyond ideas.
- Re-verify Cloud Hypervisor's license and Firecracker/ACRN architecture claims against primary sources.
- Test nested VMX under WSL2 KVM with a known-good guest hypervisor before relying on it for testing.
- Check WSL `kvm` group membership; install a Rust toolchain in WSL (owner confirmation needed, since it runs an installer).
- Hardware matrix on the ThinkPad: VT-x, EPT, VT-d, and unrestricted guest.
