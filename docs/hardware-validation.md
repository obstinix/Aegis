# Hardware validation (roadmap M5h)

M5h asks for the M3 tests to be rerun on the **ThinkPad X1 Carbon 5th Gen (i7-7600U, PyxisOS Track A)**, with the results recorded in [`research/compatibility-matrix.md`](../research/compatibility-matrix.md) as hardware-tested. This page is the checklist for doing that. [`scripts/hardware-validation.sh`](../scripts/hardware-validation.sh) runs the tests and collects the evidence.

## Three places, three kinds of evidence

| Where | What runs there | What a pass shows | Status |
|---|---|---|---|
| **GitHub Actions CI** (GitHub's own `ubuntu-24.04` and `windows-2025` runners) | fmt, clippy, unit and mock tests, docs, the `no_std`/UEFI builds, `repro-test`, the fuzz smoke tests, `cargo deny` | The code builds and its host-independent tests pass. **No live KVM test and no guest runs there** (ADR-0010). | Green since 2026-10-27. Says nothing about any hardware. |
| **ASUS ROG Strix** development workstation (Windows 11, Intel Core Ultra 9 275HX, WSL2) | The Windows suite; the WSL2 suite including the live KVM tests; `cargo xtask uefi-test` nested under WSL2; this script, to check the script itself | Aegis works under KVM inside WSL2, which is itself a VM under Hyper-V. **Not a physical-hardware result.** | Run at every milestone. |
| **ThinkPad X1 Carbon 5th Gen** (i7-7600U, Kaby Lake, 16 GB; PyxisOS Track A: Linux 6.x, UEFI, systemd-boot) | `scripts/hardware-validation.sh`, run by the owner on the machine itself | Aegis works on bare-metal KVM on the PyxisOS Track A hardware (M5h). | **Not yet run.** No result in this repository was produced on the ThinkPad. |

The script was checked on the ASUS machine, under WSL2, on 2026-10-26 (all steps passed) and again on 2026-10-27 with the two added steps. On 2026-10-27, the first run passed every REQUIRED step, but the EXTENDED `m4h-smp-virtio` step failed once on a test-timing issue; the roadmap's cross-cutting section records the details and the fix. After the fix, a rerun passed every step. That shows the script works, and that an EXTENDED failure is reported without failing the run. It is not a ThinkPad result, and `results.md` says so: on those runs it reports the CPUID hypervisor flag as present.

## Prerequisites on the ThinkPad

1. **Virtualization enabled in firmware.** In ThinkPad UEFI setup (Config → Security → Virtualization), enable Intel Virtualization Technology (VT-x), and VT-d for later milestones.
2. **KVM available.** `/dev/kvm` exists, and the `kvm_intel` module is loaded (`lsmod | grep kvm`).
3. **Access to `/dev/kvm` without root.** `ls -l /dev/kvm` shows its group, usually `kvm`. Add the user to that group, then log in again. That is an owner action; the script never uses `sudo`.
4. **Rust through rustup.** The repository pins Rust 1.97.0 in `rust-toolchain.toml`; rustup downloads it on first use, which needs network access.
5. **`nasm` and `ld`** (binutils), to build the test guests.
6. **A guest kernel as a bzImage**, passed with `--kernel`.
   - **Recommended:** copy the WSL2 kernel from the development machine (`C:\Program Files\WSL\tools\kernel`, Linux 6.18.33.2). It is the same binary all the development runs used, so any difference points at the host, not the guest. It is GPL-2.0 software used locally as test input; do not redistribute it with Aegis.
   - **Alternative:** a distribution kernel (`/boot/vmlinuz-*`). The M3 boot needs the 8250 serial console and initramfs support built in. The M4h, M6h and streaming tests also need `CONFIG_ACPI`, `CONFIG_SMP`, `CONFIG_VIRTIO_MMIO=y` and `CONFIG_VIRTIO_CONSOLE=y`, built in rather than as modules, because the tests boot with `nomodules`.
   - **Will not work:** a unified kernel image (a `.efi` under `/efi/EFI/Linux/`, common with systemd-boot), which is not a bzImage. The script checks for the bzImage header and reports a SKIP with the reason.
7. **Optional, for `b0-uefi-test`:** `qemu-system-x86_64` and OVMF (`/usr/share/ovmf/OVMF.fd`, or set `AEGIS_OVMF`). On bare metal, QEMU uses KVM directly rather than nested.
8. **A clean checkout** of the commit to validate.

## Commands

```bash
git clone https://github.com/obstinix/Aegis.git
cd Aegis
git checkout <commit to validate>
scripts/hardware-validation.sh --kernel ~/wsl-kernel
```

`--skip-extended` runs only the REQUIRED steps. Results go to `target/hardware-validation/<host>-<UTC time>/` unless `--out` is given. Archive that directory and bring it back to the development machine: `tar czf hv.tgz -C target hardware-validation`.

## Steps and expected outcomes

| Step | Level | What it runs | Pass means | Time on the dev machine (WSL2) |
|---|---|---|---|---|
| `doctor` | extended | `aegis doctor` and `--json` | The capability report was produced. Read `doctor.txt`: it should show VMX, EPT and an openable `/dev/kvm`. | ~1 s |
| `unit-tests` | **required** | `cargo test --workspace` | All unit and mock tests pass. | ~3 s (after a build) |
| `m2-kvm-guests` | **required** | the KVM guest tests (`--include-ignored`) | Real-mode guests run under KVM, with exact exit sequences, no fd leaks, and DMA permissions enforced. | ~1 s |
| `m3-linux-boot` | **required** | `boots_linux_to_the_init_marker_under_seccomp` | Linux boots to `/init` under the seccomp filter, prints `AEGIS_M3_INIT_OK`, and stops with `Shutdown`. **This is the M5h criterion.** | ~3 s |
| `m3-seccomp-selftest` | **required** | `seccomp_selftest_is_killed_by_sigsys` | A forbidden syscall kills the process with `SIGSYS`. | ~1 s |
| `m4h-smp-virtio` | extended | two vCPUs plus the virtio-console | ACPI SMP brings up 2 CPUs; the hvc0 markers arrive; virtio interrupts are counted. | ~2 s |
| `m6h-console-input` | extended | stdin to hvc0 | The guest reads both input lines. | ~3 s |
| `m6h-input-eof` | extended | stdin ending before the guest reads | Input followed at once by end of file still reaches the guest; the worker reports `ended by end of file`. | ~3 s |
| `streamed-output` | extended | streaming | The guest's prompt is seen while it runs, and the input that answers it comes back. | ~2 s |
| `streamed-output-closed-pipe` | extended | stdout closed during the boot | The guest still stops normally, `aegis` exits 0, and the summary reports `not shown: Broken pipe`. | ~3 s |
| `b0-uefi-test` | extended | `cargo xtask uefi-test` | The UEFI app prints its marker under OVMF and powers off. Exit 77 is recorded as SKIP (no QEMU or OVMF). | ~2 s |
| `repro-test` | extended | `cargo xtask repro-test` | Two clean UEFI builds are byte-identical. | ~15 s |

The script exits 0 only if every **required** step passed. A required step that is SKIPPED counts as not passed: it was not shown to work.

## Diagnostics

Each step's full output is in `<step>.log`. `environment.txt` records:
- the CPU model;
- whether `vmx`, `ept`, `vpid`, `unrestricted_guest` and the `hypervisor` flag are present;
- the `vmx flags` line and the `kvm_intel` parameters;
- the `/dev/kvm` permissions and the user's groups;
- tool versions, the commit, and the guest kernel's file type.

`dmesg-kvm.txt` keeps the KVM lines of the kernel log, if it is readable without privileges.

| Symptom | Likely cause |
|---|---|
| Steps SKIP with "/dev/kvm missing" | VT-x disabled in firmware, or `kvm_intel` not loaded. Look for `kvm: disabled by bios` in `dmesg`. |
| "/dev/kvm … not readable and writable" | The user is not in the group that owns `/dev/kvm`. |
| `m3-linux-boot` fails with no kernel banner | Not a suitable bzImage, or no 8250 console in it. Use the recommended WSL2 kernel. |
| A test dies with `Bad system call` (SIGSYS) | The host's libc issues a syscall the seccomp allowlist does not cover. Record the `type=1326 … syscall=N` line from `dmesg` (may need privileges). That line is what M3 and M4h used to find such cases. |
| `m4h-smp-virtio` fails but M3 passes | The guest kernel lacks built-in ACPI, SMP, virtio-mmio or virtio-console. |

## Recording the results

1. Copy `results.md` and the archived directory into the conversation or the devlog.
2. Update the ThinkPad row of `research/compatibility-matrix.md` from `results.md` only. Change the hosted-KVM cell from `implemented-but-untested-on-hardware` to `implemented-and-tested` only if every required step passed **and** `results.md` reports no hypervisor.
3. Mark M5h done in `research/development-roadmap.md` with the date, commit and guest kernel.

### ThinkPad results

*Not yet run.* This section is filled in from `results.md` after the script has been run on the ThinkPad.
