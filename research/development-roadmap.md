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
| **M1** | `aegis doctor` | Reports: CPU vendor/model; VMX/SVM; EPT/NPT and the VMX feature list (Linux: `/proc/cpuinfo` `vmx flags`); `/dev/kvm` presence **and whether it opens**; `KVM_GET_API_VERSION` and key `KVM_CHECK_EXTENSION` results; `kvm_intel` nested/ept/unrestricted_guest; CPU vulnerability status. `--json` emits a versioned schema. Unit tests use fixtures captured from this workstation's WSL2 (including the current "permission denied" case). On Windows it reports "hosted KVM unsupported; Hyper-V active" truthfully. | WSL2, Windows | M0 | **in progress**: all criteria met and tested on Windows and WSL2, except live `KVM_GET_API_VERSION` / `KVM_CHECK_EXTENSION`, which are implemented but have not run (blocked by `kvm` group). See M1 detail below. |
| **M2** | KVM: first guest | Creates a VM with 2 MiB of guest RAM; loads `tests/guests/hello16.asm` (real mode: `out 0x3F8` of a known string, then `hlt`); runs one vCPU. Asserts the exact IO-exit sequence and the final `Hlt`. Lifecycle reaches `Destroyed`; all fds closed (checked via `/proc/self/fd`). A negative test: a guest writing to an unassigned port is logged and the VM continues per policy; an unknown exit stops the VM with diagnostics. | WSL2 KVM | M1; **owner adds WSL user to `kvm` group** | blocked (`kvm` group) |
| **M3** | KVM: boot Linux | `linux-loader` loads a bzImage + initramfs from paths in `vm.toml`. In-kernel irqchip + PIT2; 16550 UART. The guest prints a marker from init to the serial console within a timeout. The VMM installs a seccomp allowlist after setup; a test shows a forbidden syscall kills the process. Memory cap enforced. | WSL2 KVM | M2 | blocked (`kvm` group) |
| M4h | KVM: multi-vCPU + virtio | 2+ vCPUs boot Linux; virtio-blk over MMIO with a read-only image; fuzz targets for the device register paths | WSL2 KVM | M3 | not started |
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
| `KVM_GET_API_VERSION`, `KVM_CHECK_EXTENSION` | **implemented, not run**: code compiles and passes clippy on Linux (`kvm-ioctls` 0.25.1). Runs only once `/dev/kvm` opens. Verdict logic for every result is unit-tested. |
| `kvm_intel` nested/ept/unrestricted_guest (+ enable_apicv) | **done** |
| CPU vulnerability status | **done** (Linux, passed through verbatim). Windows: not collected, and the report says so. |
| `--json` with a versioned schema | **done**: `aegis.doctor/1`; shape pinned by a test. `--raw` emits the observations used for fixtures. |
| Fixture tests from this workstation (WSL2 incl. permission denied; Windows) | **done**: 8 fixture tests; byte-exact captures, provenance in `crates/aegis-probe/tests/fixtures/PROVENANCE.md`. Mutation check: setting the fixture's `nested` to `N` fails exactly the 3 dependent tests. |
| Windows reports hosted KVM unsupported, Hyper-V active | **done**: "UNSUPPORTED" plus a note that Hyper-V (`Microsoft Hv`) hides VMX from the OS |
| Checks | fmt; 47/47 tests; clippy `-D warnings` (Windows host, Linux host, `no_std` none + uefi for `aegis-core` and `aegis-probe`); docs `-D warnings`; `cargo deny check`. All pass on Windows (1.97.0-msvc) and WSL2 (1.97.0-linux-gnu). |
| New dependencies | `serde` 1.0.229, `serde_json` 1.0.154 (pulls `zmij`, `itoa`, `memchr`), `kvm-ioctls` 0.25.1 (Linux only; pulls `kvm-bindings`, `vmm-sys-util`, `libc`, `bitflags` 1 + 2). 17 external crates, all MIT / Apache-2.0 / BSD-3-Clause. `cargo deny` warns about the duplicate `bitflags`, which comes from upstream `vmm-sys-util`. |

## Track B: bare metal (VMX, UEFI)

| # | Milestone | Acceptance criteria | Environment | Depends on | Status |
|---|---|---|---|---|---|
| **B0** | Nested test harness | `cargo xtask uefi-test` builds `aegis-uefi.efi`, creates a FAT image, boots it in WSL2 `qemu-system-x86_64 -enable-kvm -cpu host -bios OVMF`, captures serial to a file, asserts a "hello" marker, and enforces a timeout. Failure output includes the serial log. | WSL2 QEMU/KVM | M0; `kvm` group | blocked (`kvm` group) |
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
| Add the WSL user to `kvm`: `sudo usermod -aG kvm obsti`, then `wsl --shutdown` | M2, M3, B0+ |
| Approve installing Rust in WSL2 (rustup) | M0 in WSL2 |
| Approve installing `cargo-deny` (and later `cargo-fuzz`) | M0 deny check, fuzzing |
| Confirm ADR-0012 (canonical home) | README and PyxisOS docs consistency |

## Immediate next step

**M0**, which can start now on Windows: workspace, `aegis-core`, and the `no_std` build for `x86_64-unknown-uefi`. The WSL2 parts of M0, and everything from M2 on, wait for the owner actions above.
