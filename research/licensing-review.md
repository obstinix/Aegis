# Licensing Review (Phase 2)

- **Observed:** 2026-10-26. Licenses were read from repository license files, SPDX headers, and crates.io metadata for the latest published version. This is an engineering review, **not legal advice**. Items marked ⚠️ need owner review.
- **Code reused into Aegis so far:** **none.** No third-party source is committed to this repository.

---

## 1. Aegis's own license

| Item | Finding | Action |
|---|---|---|
| Intended license | README badge says Apache-2.0 | — |
| `LICENSE` file | **Missing** | ⚠️ **Owner decision.** Until a license file exists, the repository isn't licensed for anyone else to use, whatever the badge says. Adding Apache-2.0 matches PyxisOS and PyxForge, which both carry the full Apache-2.0 text (checked 2026-10-26). |
| `NOTICE` file | Not needed until Apache-2.0 code with a NOTICE (e.g. Hyperlight, Firecracker) is incorporated | Create when first needed |
| Third-party notices | None yet | If any code is adapted, add `THIRD_PARTY_NOTICES.md` and keep original headers in the adapted files |

Everything below assumes **Apache-2.0 outbound**.

---

## 2. Compatibility of candidate licenses with Apache-2.0 Aegis

| License | Seen in | Use as dependency | Adapt code into Aegis | Obligations |
|---|---|---|---|---|
| MIT | barevisor, RVM1.5, ymir, Hypervisor-101, MiniVisorPkg, illusion-rs, Bareflank, gokvm, many crates | ✅ | ✅ | Keep the copyright and permission notice |
| BSD-3-Clause | rust-vmm (`vmm-sys-util`, parts of others), crosvm, ACRN; **3 ymir files** (Andrea Orru, from "zen") | ✅ | ✅ | Keep the notice; reproduce it in binary distribution docs; no endorsement using the authors' names |
| BSD-2-Clause | bhyve | ✅ | ✅ | Keep the notice |
| Apache-2.0 | Hyperlight, Firecracker, Bao, uhyve, rust-vmm | ✅ | ✅ | Keep the notices; state changes; carry forward any `NOTICE` content |
| Dual (MIT OR Apache-2.0, Apache-2.0 OR BSD-3) | `kvm-ioctls`, `vm-memory`, `uefi`, `bitflags`, etc. | ✅ | ✅ | Pick either license; record the choice |
| Apache-2.0 **AND** BSD-3-Clause | `linux-loader`, `virtio-queue` | ✅ | ✅ | Both sets of obligations apply |
| ⚠️ NoirVisor: both `LICENSE-APACHE` and `LICENSE-MIT` present | NoirVisor | ✅ likely | ⚠️ | Whether this means "either" or applies per component wasn't confirmed. Treat as **unresolved** until checked. Ideas only for now. |
| ⚠️ Mulan PSL v2 | syswonder/hvisor (metadata only) | Probably ✅ | ⚠️ | OSI-approved permissive license; not reviewed in depth |
| CC-BY-4.0 | Cloud Hypervisor `LICENSES/` (likely docs) | n/a | Docs only, with attribution | — |
| GPL-2.0 | **Jailhouse**, **Xen**, **Linux/KVM** | ❌ for linking into Aegis | ❌ | Incorporating it would make the combined work GPL-2.0. **Ideas, papers and documented interfaces only.** |
| GPL-3.0 / AGPL-3.0 | HyperDbg, hyper-reV, Voyager, NovaHypervisor, awesome-hyper-v-exploitation (list text), others | ❌ | ❌ | Same reasoning, and Apache-2.0 → GPL-3.0 is one-way |
| **No license** | **hypv64**, **VMProtect-devirtualization**, many topic repos (see the appendix's "None" column) | ❌ | ❌ | Default copyright: all rights reserved. **Ideas only.** |

### 2.1 Using KVM and Windows APIs is not a derivative-work problem
- **KVM:** Aegis's hosted backend would talk to `/dev/kvm` only through ioctls from a userspace process. Linux's `LICENSES/exceptions/Linux-syscall-note` states that user programs using kernel services through normal system calls are not derived works of the kernel. ⚠️ Owner may want to confirm the wording against the kernel tree. It wasn't re-read this session.
- **WHP:** called through `windows-sys` (MIT OR Apache-2.0) bindings to OS-provided DLLs. No source-license issue was identified.
- **Bare-metal VMX/SVM:** implemented from the Intel SDM and AMD APM. Hardware specifications describe interfaces. Using them to implement compatible software doesn't create a copyright obligation. Don't copy code samples verbatim from manuals without checking their terms.

---

## 3. Dependency licenses (candidate crates for Aegis, from crates.io 2026-10-26)

| Crate | Latest | License | Role |
|---|---|---|---|
| `kvm-ioctls` | 0.25.1 | Apache-2.0 OR MIT | KVM ioctl wrappers |
| `kvm-bindings` | 0.14.2 | Apache-2.0 | KVM structs |
| `vmm-sys-util` | 0.15.0 | BSD-3-Clause | ioctl helpers (transitive since M2); a direct `aegis-kvm` dependency since M4h, for `SIGRTMIN` and the kick signal handler |
| `vm-memory` | 0.18.0 (in use since M2; 0.17.2 is a compatibility shim over 0.18) | Apache-2.0 OR BSD-3-Clause | Guest memory abstraction (`backend-mmap`) |
| `thiserror`, `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | Pulled in by `vm-memory` |
| `winapi` (+ two `-gnu` helper crates) | 0.3.9 / 0.4.0 | MIT OR Apache-2.0 | Locked as `vm-memory`'s Windows-only dependency; never compiled, since `aegis-kvm` builds `vm-memory` only on Linux |
| `linux-loader` | 0.14.0 | Apache-2.0 AND BSD-3-Clause | **Not used** (M3 decision, ADR-0004 note) |
| `seccompiler` | 0.5.0 | Apache-2.0 OR BSD-3-Clause | Seccomp BPF filter for the run loop (M3) |
| `toml` (+ `toml_parser`, `toml_datetime`, `toml_writer`, `serde_spanned`) | 1.1.x | MIT OR Apache-2.0 | `vm.toml` parsing (M3) |
| `winnow` | 1.0.4 | MIT | TOML parser internals |
| `indexmap`, `hashbrown`, `equivalent` | 2.14.2 / 0.17.1 / 1.0.3 | MIT OR Apache-2.0 (indexmap: Apache-2.0 OR MIT) | Pulled in by `toml` |
| `virtio-queue` | 0.18.0 | Apache-2.0 AND BSD-3-Clause | **Not used** (M4h decision, ADR-0008 note): `aegis-devices` has its own `no_std` split virtqueue |
| `vm-device` | 0.1.0 | Apache-2.0 OR BSD-3-Clause | Device bus |
| `seccompiler` | 0.5.0 | Apache-2.0 OR BSD-3-Clause | Seccomp filters for the VMM |
| `event-manager` | 0.4.2 | Apache-2.0 OR BSD-3-Clause | epoll loop |
| `acpi_tables` | 0.2.1 | Apache-2.0 | ACPI table and AML generation (in use since M4h, Linux only; contains no `unsafe`) |
| `zerocopy`, `zerocopy-derive` | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | Pulled in by `acpi_tables` (M4h) |
| `syn` 2 | 2.0.119 | MIT OR Apache-2.0 | Build-time only, for `zerocopy-derive`. It duplicates `syn` 3.0.9 (used by `serde_derive` and `thiserror-impl`); `cargo deny` reports this as a warning, since `multiple-versions = "warn"`. Not in any binary. |
| `mshv-ioctls` | 0.7.1 | Apache-2.0 OR BSD-3-Clause | MSHV backend (optional, later) |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | WHP backend (later) |
| `uefi` | 0.41.0 | MIT OR Apache-2.0 | UEFI app for the bare-metal core |
| `x86_64` | 0.16.0-rc.0 | MIT/Apache-2.0 | Paging/registers (bare metal) |
| `x86` | 0.52.0 | MIT | VMX/MSR definitions (last release 2022-10, so maintenance risk) |
| `raw-cpuid` | 11.6.0 | MIT | CPUID decoding |
| `bitflags`, `log`, `thiserror`, `libc`, `spin` | — | MIT OR Apache-2.0 (spin: MIT) | General |

**All are compatible with Apache-2.0 outbound.** Transitive dependencies weren't resolved, because no `Cargo.lock` exists yet. Phase 4 adds `cargo-deny` with an allowlist (MIT, Apache-2.0, BSD-2/3, ISC, Unicode-3.0, Zlib), so a transitive GPL or no-license crate fails the build. Installing `cargo-deny` needs owner approval.

---

## 4. What may be reused, and how

| Source | Reusable | Not reusable |
|---|---|---|
| Intel SDM / AMD APM / UEFI spec / KVM API docs / Hyper-V TLFS / WHP docs | Interfaces, encodings, field layouts, algorithms | Verbatim long passages in docs |
| barevisor, RVM1.5, Hypervisor-101, ymir (MIT) | Ideas freely. Code **only with** the MIT notice preserved in the adapted file and a `THIRD_PARTY_NOTICES.md` entry. | — |
| ymir `idt.zig`, `interrupt.zig`, `isr.zig` | Ideas. Code only with **both** ymir's MIT notice and the embedded BSD-3 (Andrea Orru) notice. ymir is Zig, so direct reuse is unlikely anyway. | — |
| Jailhouse, Xen, KVM (GPL-2.0) | Design ideas, papers, public interfaces | Any code, including "translated" code |
| hypv64, VMProtect-devirtualization, other unlicensed repos | Ideas from reading | Any code |
| Hyperlight, Firecracker, Cloud Hypervisor (Apache-2.0 / BSD-3) | Ideas. Code with notices and NOTICE propagation. Prefer depending on published crates over copying. | — |

**Clean-room practice for the bare-metal core:** write against the manuals. When a reference project is consulted, record which one and why in the commit message or devlog. Never paste code from GPL or unlicensed sources, even temporarily.

## 5. Attribution record

| Adapted file in Aegis | Origin | License | Notice preserved |
|---|---|---|---|
| *(none yet)* | | | |
