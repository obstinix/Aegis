# Licensing Review (Phase 2)

- **Observed:** 2026-10-26; §3.1 and §3.2 on 2026-10-27. Licenses were read from repository license files, SPDX headers, and crates.io metadata for the latest published version. This is an engineering review, **not legal advice**. Items marked ⚠️ need owner review.
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
| `uefi` | 0.41.0 | MIT OR Apache-2.0 | **Not used** (B0): it always depends on `ucs2`, below. Owner decision pending on allowing that. |
| `ucs2` | 0.3.3 | **MPL-2.0** | **Not used.** Outside the allowlist. MPL-2.0 is file-level copyleft: using it unmodified as a dependency is compatible with Apache-2.0 outbound, but distributed binaries must point to its source, and modified files stay MPL-2.0. Allowing it needs an explicit `deny.toml` exception, which is the owner's call. Source inspection and options: §3.1. |
| `uefi-raw` | 0.17.0 | MIT OR Apache-2.0 | Raw UEFI type definitions for `aegis-uefi` (B0, UEFI target only) |
| `uguid` | 2.2.1 | MIT OR Apache-2.0 | Pulled in by `uefi-raw` |
| `x86_64` | 0.16.0-rc.0 | MIT/Apache-2.0 | Paging/registers (bare metal) |
| `x86` | 0.52.0 | MIT | VMX/MSR definitions (last release 2022-10, so maintenance risk) |
| `raw-cpuid` | 11.6.0 | MIT | CPUID decoding |
| `bitflags`, `log`, `thiserror`, `libc`, `spin` | — | MIT OR Apache-2.0 (spin: MIT) | General |

**All are compatible with Apache-2.0 outbound.** This table was first written before a `Cargo.lock` existed. Transitive dependencies are now checked mechanically: `cargo deny check` enforces the ADR-0011 allowlist (MIT, Apache-2.0, BSD-2/3, ISC, Unicode-3.0, Zlib) over the whole locked graph with all features, locally and in CI on every push, so a transitive GPL, MPL or unlicensed crate fails the check.

### 3.1 `ucs2` 0.3.3: source inspection (2026-10-27)

What was checked: the crate source as published on crates.io, fetched into a scratch project outside the repository (nothing was added to Aegis), plus `uefi` 0.41.0's manifest and source.

| Question | Finding |
|---|---|
| Version | 0.3.3, the latest release. `uefi` 0.41.0 (latest) requires `ucs2 = "0.3.3"`. |
| Licence metadata | `license = "MPL-2.0"` in `Cargo.toml`. Repository `rust-osdev/ucs2-rs`; published from commit `aa837529a4999e8c7eacb326fd153cc52792814b`. |
| Licence text | `LICENSE` is the full, unmodified MPL-2.0 text. |
| Source file headers | None. Neither `src/lib.rs` (227 lines) nor `src/macros.rs` (126 lines) has a licence header. No file carries the Exhibit B "Incompatible With Secondary Licenses" notice; that phrase appears only in the standard licence text itself. |
| Its own dependencies | `bit_field` 0.10.3 (Apache-2.0/MIT): allowed. |
| Optional in `uefi`? | **No.** `ucs2` is an unconditional dependency of `uefi` 0.41; no feature removes it. |
| How `uefi` uses it | In 4 source files. `uefi` re-exports the `ucs2_cstr!` macro, which its own `cstr16!` macro expands to. The text-output protocol's `fmt::Write` implementation calls `ucs2::encode_with`. Both are small UCS-2 encoding helpers. |
| What Aegis uses today | Nothing. `aegis-uefi` uses `uefi-raw` 0.17.0 (MIT OR Apache-2.0) and does its own UCS-2 conversion. `ucs2` is not in `Cargo.lock`. |

**What allowing it would mean.** MPL-2.0 obligations attach to the `ucs2` files, not to Aegis:
- Aegis stays Apache-2.0 as a "Larger Work" (MPL-2.0 §3.3).
- Anyone distributing an `aegis-uefi` binary must tell recipients where to get `ucs2`'s source (§3.2). A `THIRD_PARTY_NOTICES.md` entry naming the crate, version and repository would cover it.
- A modified copy of a `ucs2` file must stay MPL-2.0, with its source available. Using the crate unmodified from crates.io avoids that.

**Options for the owner.** None of these has been applied, and `deny.toml` is unchanged.

1. **Keep `uefi-raw` only (current state).** No licence change; `aegis-uefi` keeps hand-written wrappers. This is cheap while the UEFI app only prints and powers off. The cost grows with every UEFI protocol later milestones need (memory map, file system, ACPI tables).
2. **Allow `ucs2` alone and switch to `uefi`.** Add a crate-scoped exception, so that MPL-2.0 does not become generally allowed:
   ```toml
   [[licenses.exceptions]]
   crate = "ucs2"
   allow = ["MPL-2.0"]
   ```
   Then add `THIRD_PARTY_NOTICES.md` before the first binary is distributed, and record the decision as an ADR-0011 note.
3. **Replace `ucs2` under `uefi`** with a `[patch]` reimplementation. Not recommended: it means maintaining a fork of the crate's API. Cleanly reimplementing a 353-line encoder is also more work than option 1.

**Recommendation:** option 1 until the UEFI side needs protocols that would make hand-written wrappers costly; decide then between 1 and 2. This is an engineering assessment, not legal advice. The obligations, compatibility and alternatives are summarized for the decision in [`ucs2-licensing-decision.md`](ucs2-licensing-decision.md).

### 3.2 Fuzz-only dependencies (2026-10-27)

The `fuzz/` crate (docs/fuzzing.md) is a separate Cargo workspace. Its dependencies are not part of any Aegis binary, and the main workspace's `cargo deny check` does not see them.

| Crate | Version | License | Role |
|---|---|---|---|
| `libfuzzer-sys` | 0.4.13 | **(MIT OR Apache-2.0) AND NCSA** (crate metadata) | cargo-fuzz targets only (`--features libfuzzer`). It bundles LLVM's libFuzzer C++ sources; see the note below. |
| `arbitrary` | 1.5.0 | MIT OR Apache-2.0 | Pulled in by `libfuzzer-sys` |
| `cc`, `find-msvc-tools`, `jobserver`, `shlex` | 1.10.0 / 0.1.15 / 0.1.35 / 2.0.1 | MIT OR Apache-2.0 | Build-time: compile libFuzzer |
| `getrandom`, `cfg-if` | 0.4.3 / 1.0.5 | MIT OR Apache-2.0 | Pulled in through `jobserver` |

**`libfuzzer-sys`'s licence, as inspected.**
- Its `README.md` says: all files in the `libfuzzer` directory are NCSA, and everything else is Apache-2.0 and MIT. The crate metadata follows the README.
- The bundled files say otherwise. 55 of the 56 files under `libfuzzer/` carry `SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception`, LLVM's licence since its relicensing. The remaining file is `README.txt`, with no tag. No file there mentions NCSA.
- So the metadata looks out of date. Either way, both licences are permissive: NCSA is the University of Illinois/NCSA licence, BSD-style, and the LLVM exception is to Apache-2.0. The ADR-0011 allowlist names neither NCSA nor `Apache-2.0 WITH LLVM-exception`.
- cargo-deny goes by the metadata. Running `cargo deny --manifest-path fuzz/Cargo.toml --all-features check licenses` on 2026-10-27 rejected exactly one crate, `libfuzzer-sys`, for NCSA. The fuzzer is a development tool and is never linked into Aegis, so the allowlist, which governs what Aegis ships, does not need to change. If the owner wants the fuzz crate held to the same check, the narrow fix is a `libfuzzer-sys`-scoped exception in a fuzz-only deny configuration.

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
