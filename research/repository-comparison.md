# Repository Comparison and Ranking (Phase 2)

- **Observed:** 2026-10-26. Builds on [`hypervisor-landscape.md`](hypervisor-landscape.md).
- **Evidence base:** shallow clones (source read, grep for mechanisms, test/CI files, license files and headers), GitHub API metadata, crates.io license metadata. **No candidate was built or run.** "Testing" scores reflect test and CI *files present*, not observed pass rates.
- Firecracker, Cloud Hypervisor, crosvm, ACRN, Xen and bhyve are **not scored**: their source was not read this session. They appear in §4 as documentation-based references only.

---

## 1. Methodology

### 1.1 Gates (pass/fail, applied before scoring)

| Gate | Requirement | Why |
|---|---|---|
| G1 | Supports x86-64 hardware virtualization (VMX and/or SVM, directly or through KVM/MSHV/WHP) | Aegis's stated target. |
| G2 | License permits the *intended use*: dependency, or adapted code in an Apache-2.0 project | §5 of the directive. Failing G2 doesn't stop a project being an *ideas-only* reference. |

A project failing a gate can still be an architectural reference, but it cannot be a **foundation**, whatever its score.

### 1.2 Weighted criteria (each scored 1–5; total scaled to 100)

| Criterion | Weight | What a 5 means for *Aegis* |
|---|---|---|
| Architectural suitability | 20% | Rust; x86-64; runs *its own* guests with a defined VM lifecycle (not only blue-pilling the running OS); fits a staged hosted → bare-metal plan |
| Hardware/platform compatibility | 20% | Intel and AMD; works on the hosts actually available (§7 of the landscape report) |
| Security & isolation | 15% | Explicit guest/host isolation, IOMMU/DMA awareness, attack-surface minimization, sandboxing of the VMM |
| Extensibility & maintainability | 15% | Modular; readable; reasonable size; active |
| Implementation completeness | 10% | Boots real guests and handles real exits, rather than demos |
| Build/test/debug quality | 10% | Unit/integration tests, CI, fuzzing, emulator test harness |
| License suitability | 10% | Permissive and compatible with Apache-2.0 outbound, clear headers |

**Limitations.** Scores are one reviewer's judgment from partial source reading. A ±1 shift on any criterion moves a total by 2–4 points, so differences under about 5 points are not meaningful. Stars were recorded but deliberately **not** used as a criterion.

---

## 2. Comparison table

| Project | Popularity (stars, 2026-10-26) | License | Project type | Architecture | Virtualization | Boot | Host | Guests | Devices | Isolation | Testing (files observed) | Maintenance (last push) | Dependencies | Compatibility | Extensibility | Licensing risk | Feasibility | Recommendation |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| [rust-vmm](https://github.com/rust-vmm/rust-vmm) | 148 (monorepo) | Per crate: Apache-2.0 / BSD-3 / dual | Production building blocks | x86-64, aarch64, riscv64 | Via KVM (and MSHV in sibling crates) | n/a (library); `linux-loader` loads bzImage/ELF | Linux | Whatever the VMM built on it supports | `vm-virtio`/`virtio-queue`, `vm-device` buses | KVM boundary + `seccompiler` | 127 test files; release CI per crate | 2026-10-19 | `libc`, `vmm-sys-util`, `kvm-bindings` | Any KVM-capable Intel/AMD host | Very high (crates) | Low | Low effort to integrate | **Foundation** (as dependencies, not a fork) |
| [Hyperlight](https://github.com/hyperlight-dev/hyperlight) | 4,758 | Apache-2.0 + NOTICE | Production embedded micro-VM | x86-64 (aarch64 partial) | KVM, MSHV, WHP | Host loads a purpose-built guest binary | Linux, Windows | **Hyperlight guests only** (no general OS) | None by design | Minimal surface; fuzzing, miri CI | 95 test files; ~30 workflows incl. fuzzing, audit, coverage | 2026-10-26 | Large (tokio, tracing, opentelemetry, flatbuffers…) | KVM/MSHV/WHP hosts | Medium (opinionated model) | Low | n/a as fork | **Reference** for multi-backend (KVM+WHP) design |
| [NoirVisor](https://github.com/Zero-Tang/NoirVisor) | 658 | Apache-2.0 and MIT license files both present | Research | x86-64 | VMX+EPT, SVM+NPT, nested virt | UEFI / Windows driver | Bare metal under Windows | Running OS (blue-pill); CVM work | Few | IOMMU/DMA code present | 3 test files; Bochs + QEMU CI | 2026-10-25 | Few (`r-efi`, `spin`…) | Intel, AMD, Hygon, Zhaoxin (per README) | Low (large C core, idiosyncratic) | Medium (dual-license intent unclear) | High effort | Reference |
| [Jailhouse](https://github.com/siemens/jailhouse) | 1,975 | **GPL-2.0** (+ hypercall exception) | Production partitioning | x86-64, ARM, ARM64 | VMX+EPT, SVM+NPT, VT-d, AMD-Vi, x2APIC, posted interrupts | Loaded from a running Linux | Linux | Linux, RTOS, bare-metal cells | Minimal; passthrough of partitioned devices | **Strong**: static partitioning + IOMMU | 0 unit-test files found; CI build workflow | 2024-05-18 | Linux kernel tree | Real hardware (per project) | Low (C, kernel-coupled) | **High: GPL-2.0** | n/a | **Reference, ideas only** (fails G2) |
| [RVM1.5](https://github.com/rcore-os/RVM1.5) | 73 | MIT | Research ("type-1.5") | x86-64 | VMX+EPT, SVM+NPT, partial AMD IOMMU | Loaded from Linux kernel module | Linux | Linux (demoted into a cell) | Minimal | Cell-based | 2 test files; CI `main.yml` | 2024-03-06 | `x86`, `x86_64`, `raw-cpuid`, `spin`, allocators | QEMU (per README) | Medium (`libvmm` typed VMCS/VMCB crate) | Low | n/a | **Reference** (Rust VMX/SVM structures) |
| [uhyve](https://github.com/hermit-os/uhyve) | 296 | Apache-2.0 / MIT | Specialized hosted VMM | x86-64, aarch64 | KVM (+ macOS HVF) | Loads Hermit unikernels | Linux, macOS | **Hermit only** | Minimal | KVM boundary | 22 test files; CI | 2026-10-10 | rust-vmm crates | KVM hosts | Medium | Low | n/a | Reference |
| [barevisor](https://github.com/tandasat/barevisor) | 385 | MIT (no per-file headers) | Educational | x86-64 | VMX+EPT, SVM+NPT | UEFI driver / Windows driver | Bare metal, blue-pill | The running OS | None | **None by design**: README says it "does not attempt to protect itself from the guest or DMA" | 3 test files; Bochs/VMware `xtask` harness; no CI | 2026-08-02 | `x86`, `raw-cpuid`, `uefi`, `wdk-*`, `spin` | "Select hardware models" (README) | **High** (clean Intel/AMD trait split in stable Rust) | Low | n/a | **Reference** (bare-metal Rust structure) |
| [gokvm](https://github.com/bobuhiro11/gokvm) | 267 | MIT | Educational hosted VMM | x86-64 | KVM | bzImage + initrd | Linux | Linux | serial, virtio-net/blk | KVM boundary | 17 test files; CI | 2026-08-19 | none external (Go) | KVM hosts | Medium | Low | n/a | Reference (minimal Linux boot over KVM) |
| [ymir](https://github.com/smallkirby/ymir) | 69 | MIT; 3 files carry **BSD-3 notices** (Andrea Orru, "zen") | Educational type-1 | x86-64 (Intel only) | VMX+EPT | UEFI loader `surtr` → hypervisor | Bare metal | Linux 6.9 + initramfs | serial, minimal I/O | Minimal | 5 test files; CI test/lint/boot | 2026-10-16 | Zig toolchain | Intel only; **no SMP, no APIC** (README) | Medium | Low (preserve BSD-3 notices) | n/a | **Reference** (UEFI → own-guest boot flow) |
| [Bareflank](https://github.com/Bareflank/hypervisor) | 1,518 | MIT | SDK | x86-64 | VMX+EPT, SVM | Windows/Linux/UEFI loaders | Bare metal | Running OS | — | — | 1 test file found; CI | 2024-08-14 | Bareflank BSL etc. | Intel/AMD | Low (202k LoC, C++) | Low | High | Reject as base |
| [Hypervisor-101-in-Rust](https://github.com/tandasat/Hypervisor-101-in-Rust) | 1,213 | MIT | Course / fuzzer | x86-64 | VMX+EPT, SVM+NPT | UEFI app | Bare metal | Snapshot-restored code | — | — | 2 test files; Bochs/VMware harness | 2026-10-11 | `uefi`, `x86` | Intel/AMD | Low (special purpose) | Low | n/a | Learning resource |
| [MiniVisorPkg](https://github.com/tandasat/MiniVisorPkg) | 762 | MIT | Research | x86-64 (Intel) | VMX+EPT | EDK2 UEFI / Windows | Bare metal | Running OS | — | — | 0 test files found; no CI | 2024-08-15 | EDK2 | Intel only | Low | Low | n/a | Learning resource |
| [illusion-rs](https://github.com/memN0ps/illusion-rs) | 413 | MIT | Research (stealth EPT hooks) | x86-64 (Intel) | VMX+EPT | UEFI blue-pill | Bare metal | Windows | — | Purpose is hiding hooks, not isolation | 1 test file; CI | 2026-05-14 (**archived**) | `x86`, `uefi` | Intel only | Low | Low | n/a | **Reject** (purpose mismatch, archived) |
| [Bao](https://github.com/bao-project/bao-hypervisor) | 585 | Apache-2.0 | Production static partitioning | ARM, RISC-V (+ others), **no x86** | Stage-2 / SMMU | Bare metal | — | Linux, RTOS | Minimal | Strong | 0 unit-test files; build CI per arch | 2026-10-22 | — | Not x86 | — | Low | n/a | Reference (fails G1) |
| [hypv64](https://github.com/mooofin/hypv64) | 23 | **None** | Hobby | RISC-V 64 | H-extension (`hgatp`) | QEMU `virt` | — | Linux (panics on first trap, per README) | None | None | 0; no CI | 2026-05-29 | — | RISC-V QEMU only | — | **Cannot reuse** | n/a | **Reject** (fails G1 and G2) |

---

## 3. Weighted scores

| Rank | Project | G1 | G2 | Arch (20) | HW (20) | Sec (15) | Ext (15) | Compl (10) | Test (10) | Lic (10) | **Score /100** |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | rust-vmm | ✅ | ✅ | 4 | 4 | 4 | 5 | 3 | 5 | 5 | **85** |
| 2 | Hyperlight | ✅ | ✅ | 3 | 4 | 5 | 3 | 4 | 5 | 5 | **80** |
| 3 | NoirVisor | ✅ | ✅ (verify) | 3 | 4 | 3 | 2 | 4 | 3 | 4 | **65** |
| 4 | Jailhouse | ✅ | ❌ code | 3 | 4 | 5 | 2 | 4 | 2 | 1 | **63** |
| 5= | RVM1.5 | ✅ | ✅ | 3 | 3 | 3 | 3 | 3 | 2 | 5 | **62** |
| 5= | uhyve | ✅ | ✅ | 2 | 3 | 3 | 3 | 3 | 4 | 5 | **62** |
| 7 | barevisor | ✅ | ✅ | 4 | 3 | 1 | 4 | 2 | 2 | 5 | **61** |
| 8 | gokvm | ✅ | ✅ | 2 | 3 | 2 | 3 | 3 | 4 | 5 | **59** |
| 9 | ymir | ✅ | ✅ | 3 | 2 | 2 | 3 | 3 | 4 | 4 | **57** |
| 10 | Bareflank | ✅ | ✅ | 2 | 3 | 3 | 3 | 3 | 2 | 4 | **56** |
| 11 | Hypervisor-101-in-Rust | ✅ | ✅ | 2 | 3 | 1 | 2 | 2 | 2 | 5 | **47** |
| 12 | MiniVisorPkg | ✅ | ✅ | 2 | 2 | 2 | 2 | 3 | 1 | 5 | **46** |
| 13 | illusion-rs | ✅ | ✅ | 2 | 2 | 1 | 2 | 3 | 2 | 5 | **45** |
| — | Bao | ❌ | ✅ | gated | | | | | | | — |
| — | hypv64 | ❌ | ❌ | gated | | | | | | | — |

How to read this: the bare-metal candidates cluster in the 55–65 band. Within that band the differences aren't meaningful (§1.2), and none is mature enough to adopt. The two hosted options score clearly higher because they are tested, maintained, and run on hardware Aegis can use today.

---

## 4. Documentation-based references (not scored)

| Project | Why it matters | Verify before relying on |
|---|---|---|
| Firecracker (Apache-2.0, 37.6k ★) | Minimal virtio device set; jailer; seccomp per thread | Device list, jailer model |
| Cloud Hypervisor (`LICENSES/`: Apache-2.0, BSD-3-Clause, CC-BY-4.0 files observed) | Same rust-vmm base; KVM **and** MSHV backends behind one `hypervisor` crate | Backend trait design |
| crosvm (BSD-3) | Per-device sandboxed processes (privilege separation) | Minijail usage |
| ACRN (BSD-3) | Permissive x86 type-1 with a service VM | Activity: last push 2026-04-06 |
| bhyve (BSD-2) | Permissive VMX/SVM kernel module + userspace device model | — |
| Xen (GPL-2.0) | XSM/FLASK, grant tables, driver domains | Ideas only |

---

## 5. Answers to the six questions

1. **Strongest foundation:** the **rust-vmm crates on Linux KVM** (`kvm-ioctls`, `kvm-bindings`, `vm-memory`, `linux-loader`, later `virtio-queue`, `seccompiler`). Use them as versioned dependencies, not a fork. They are permissively licensed, tested, and the base of Firecracker and Cloud Hypervisor. They run on hardware available today (WSL2 KVM) and on PyxisOS Track A.
2. **Strongest architectural reference:**
   - *Hosted:* **Hyperlight**, for one Rust codebase with KVM, MSHV and WHP backends. **Cloud Hypervisor** (documentation) for the backend trait.
   - *Bare-metal:* **barevisor** for the Intel/AMD split in stable Rust; **RVM1.5 `libvmm`** for typed VMCS/VMCB; **ymir** for the UEFI-loader → own-guest → Linux boot flow; **Jailhouse** (ideas only, GPL-2.0) for IOMMU/interrupt partitioning.
3. **Best starting point for an experimental x86-64 core:** a **clean-room Rust implementation** written against the Intel SDM Vol. 3C and AMD APM Vol. 2, with the references above. Its first target is a UEFI application on OVMF, run as an L2 guest under QEMU/KVM in WSL2 (nested VMX: `nested=Y`, EPT and unrestricted guest observed). No candidate is a better place to *start from* than a clean slate, because each one bakes in a blue-pill or special-purpose model that Aegis would have to remove.
4. **Must not be copied or forked:**
   - **hypv64**: no license, and RISC-V.
   - **Jailhouse, Xen, Linux KVM source**: GPL-2.0, incompatible with Apache-2.0 Aegis for incorporated code.
   - **HyperDbg, HyperHide-class tools, Voyager, and other GPL-3.0/AGPL repos**: license, and/or offensive purpose.
   - **illusion-rs** and stealth/anti-cheat-bypass hypervisors: built for hiding hooks, opposite to Aegis's isolation goal; illusion-rs is archived.
   - **barevisor as a base**: it explicitly provides no protection from the guest or DMA. Use it for structure, never for its trust model.
   - **VMProtect-devirtualization**: no license, and unrelated.
5. **Bare-metal, hosted, KVM-based, or staged?** **Staged (Option C).** Hosted on KVM first, so Aegis can actually create, run, monitor and stop VMs, with tests. The bare-metal VMX core is developed in parallel behind the same vCPU, guest-memory and exit-handler interfaces. WHP is a later hosted backend for this Windows workstation. Final decision and ADR in Phase 3.
6. **Most realistic path to a secure, functional first release:** see the draft milestones in [`compatibility-matrix.md`](compatibility-matrix.md) §5; Phase 3 finalizes them. In outline:
   - `aegis doctor`: capability detection that tells you the truth.
   - A KVM backend running a tiny test guest to `HLT`, with exit accounting.
   - Booting a Linux bzImage to a serial console under seccomp.
   - In parallel, the bare-metal core reaching VMXON, then VMLAUNCH of a CPUID/HLT guest, nested.
   - The first tagged release covers only the hosted path, labeled with what was tested where.
