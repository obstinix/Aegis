# Compatibility Matrix (Phase 2)

- **Observed:** 2026-10-26.
- **Status vocabulary (every entry uses one):** `implemented-and-tested` · `implemented-but-untested-on-hardware` · `experimental` · `planned` · `unsupported`.
- **Aegis has no code**, so every Aegis capability below is `planned` or `unsupported`. The "Environment facts" columns record what was **observed** about each platform, which is separate from what Aegis does on it.
- Emulator success will never be reported as physical-hardware support.

---

## 1. Platforms and environments

| Environment | Environment facts (observed 2026-10-26 unless noted) | Hosted KVM backend | Bare-metal VMX core | Bare-metal SVM core | Hosted WHP backend |
|---|---|---|---|---|---|
| **Dev workstation, Windows 11 root partition**: Intel Core Ultra 9 275HX | `HypervisorPresent=True` (Hyper-V/VBS active). WMI reports VMX/SLAT `False`, as Windows hides them from the root partition. QEMU 11.1.0 with accelerators `tcg`, `whpx`; edk2 x86_64 firmware bundled. | unsupported (no KVM on Windows) | **unsupported** (VMX hidden; disabling Hyper-V/VBS is a security setting change, not recommended) | unsupported (Intel CPU) | planned. ⚠️ Whether the WHP optional feature is enabled wasn't checked (needs elevation). QEMU lists `whpx`, which suggests the API is present. |
| **WSL2 Ubuntu on the workstation** (L1 under Hyper-V): kernel 6.18.33.2-microsoft-standard-WSL2, 24 vCPUs, ~15.3 GiB | `/dev/kvm` present (`root:kvm 660`). `kvm_intel`: `nested=Y`, `ept=Y`, `unrestricted_guest=Y`. VMX flags exposed: `vnmi invvpid ept_x_only ept_ad ept_1gb tsc_offset vtpr ept vpid unrestricted_guest ept_mode_based_exec tsc_scaling usr_wait_pause`. **Not exposed:** APICv / posted interrupts / VMCS shadowing / PML. QEMU 8.2.2; OVMF installed. **User `obsti` was not in group `kvm` at first ("Permission denied"); the owner added it on 2026-10-26, and `/dev/kvm` now opens (API 12; irqchip, user_memory, pit2, ext_cpuid, immediate_exit).** Rust 1.97.0 installed since M0. | planned. `/dev/kvm` access verified (M1). | planned, as an **L2 guest** of QEMU/KVM (`-cpu host`, OVMF). ⚠️ The triple-nested path (Hyper-V → WSL2 KVM → L2 Aegis → L3 guest) is untested. | unsupported here (KVM on Intel can't expose SVM) | n/a |
| **QEMU TCG (no acceleration)**, Windows or WSL | ⚠️ Believed: QEMU TCG emulates SVM (incl. NPT) but **not** VMX. To be verified. | n/a | unsupported (no TCG VMX) | planned, as the **only available AMD-V test path** (no AMD hardware). To verify. | n/a |
| **PyxisOS Track A hardware**: ThinkPad X1 Carbon 5th Gen, i7-7600U (Kaby Lake), 16 GB | From PyxisOS docs: Linux 6.x, UEFI, systemd-boot. VT-x + EPT + VT-d expected for this CPU model, **not verified on the machine.** | planned (`implemented-but-untested-on-hardware` until run there) | planned (physical validation is a late milestone) | unsupported | n/a |
| AMD hardware | None available | planned (KVM is vendor-neutral) | n/a | planned, **untested on hardware** | — |
| ARM64 / RISC-V | Out of scope | unsupported | unsupported | unsupported | unsupported |

## 2. Feature dimensions (target first release vs later)

| Dimension | First release target | Status |
|---|---|---|
| CPU vendors | Intel (tested via KVM), AMD (KVM; not tested on hardware) | planned |
| Intel VMX (bare-metal core) | VMXON / VMCS / VMLAUNCH / exits: CPUID, HLT, I/O, EPT violation | planned (experimental track) |
| AMD SVM (bare-metal core) | Same subset, via TCG first | planned (later than VMX) |
| EPT / NPT | Required. Identity or explicit GPA→HPA maps; 4 KiB + 2 MiB pages | planned |
| Firmware / boot | Hosted: n/a. Bare metal: UEFI only (OVMF in QEMU). **Legacy BIOS: unsupported.** | planned / unsupported |
| IOMMU (VT-d / AMD-Vi) | **No device passthrough in the first release.** Without IOMMU enforcement, passthrough is unsafe. | unsupported (until a dedicated milestone) |
| Interrupt controller | Hosted: KVM in-kernel irqchip (`KVM_CREATE_IRQCHIP`). Bare metal: no APICv dependency (it isn't available nested here). Software LAPIC/x2APIC exits later. | planned |
| SMP | Hosted: multi-vCPU later. Bare metal: BSP only at first. | planned |
| Memory requirements | Guest RAM cap enforced per VM (default small, e.g. 256 MiB; configurable) | planned |
| Guests | First: tiny purpose-built test guests (real-mode/long-mode stubs). Then: Linux bzImage + initramfs, serial console. | planned |
| Guest: PyxisOS Track B kernel (Multiboot 1) | Possible with a Multiboot loader or PVH entry. PyxisOS boots via QEMU `-kernel` today. | planned (separate validation) |
| Emulators | QEMU/KVM (WSL2), QEMU TCG (SVM), Bochs (as used by barevisor) optional | planned |
| Host OS | Linux with KVM ≥ the version validated (6.18 WSL2 observed). Windows (WHP) later. | planned |

## 3. PyxisOS: the four compatibility goals, kept separate

| Goal | What it requires | Current evidence | Status |
|---|---|---|---|
| 1. Aegis runs **on** PyxisOS as host | Track A: it's Linux with KVM, so the hosted backend applies directly. Track B: needs its own kernel to host the bare-metal core, which is far off (Lunar Core halts after GDT setup). | Track A Linux is verified working on hardware (per PyxisOS docs). | Track A: planned. Track B: unsupported for now. |
| 2. PyxisOS runs **as a guest** under Aegis | Track A: a Linux bzImage guest, a generic capability. Track B kernel: Multiboot 1 → needs a Multiboot/PVH loader in Aegis. | PyxisOS Track B boots in QEMU via `-kernel`. | planned |
| 3. Shared components/interfaces | PyxisOS `native/aegis` has a capability/sandbox model with 4 tests; its VT-x detection is a stub returning `false`. Candidate: Aegis provides a real `capability` probe crate that PyxisOS could depend on later. | Overlap in name and intent (owner decision pending on which is canonical). | planned. **No PyxisOS change** without the owner. |
| 4. Aegis boots **independently** of PyxisOS | The bare-metal core is a standalone UEFI application; the hosted backend runs on any KVM Linux. | — | planned (by design) |

Constraint: nothing in Aegis's plan requires modifying PyxisOS. The `pyxis` CLI could later call `aegis doctor` as an external command.

## 4. PyxForge integration assessment

| Possible integration | Fits PyxForge's actual architecture? | Recommendation |
|---|---|---|
| Aegis `pyxforge.toml` with build/test profiles | ✅ Uses existing `internal/build` profiles | **Do first** (ships in the Aegis repo; no PyxForge code change) |
| QEMU launch configs for the L2 bare-metal core (OVMF, `-cpu host`, `-s -S`) | ✅ Existing `internal/qemu` runner | Yes, as profiles |
| GDB debugging of the bare-metal core | ✅ Existing `internal/gdb` | Yes. Needs an ELF/PDB with symbols from the UEFI build. |
| VM logs / diagnostic inspection | ✅ Viewing files and serial output already exists | Aegis writes structured logs (JSON lines) that PyxForge can open as text |
| Hypervisor config editing | ✅ It's TOML in Neovim | No special support needed |
| Architecture-aware navigation, Aegis templates, artifact management, hardware reports | Possible, but each is new Go/Fyne code in PyxForge | **Later**, and only on owner request. Must stay native Go + Fyne, no web UI. |

## 5. Draft milestone ladder (input to Phase 3)

| # | Milestone | Acceptance criterion (proposed) | Runs on |
|---|---|---|---|
| M0 | Repo skeleton | `cargo build`, `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`, `cargo deny check` pass in CI | Any |
| M1 | `aegis doctor` | Reports CPUID VMX/SVM, `/dev/kvm` presence **and openability**, KVM capabilities, nested params. Correct on WSL2 (currently: "kvm present, permission denied"). Unit tests over captured CPUID/sysfs fixtures. | WSL2, Linux |
| M2 | KVM: run a test guest | Create VM, map guest memory, load a 16-bit stub that does `out` + `hlt`; assert the exit sequence; clean teardown; no leaked fds | WSL2 KVM |
| M3 | KVM: boot Linux | bzImage + initramfs reaches userspace and prints to the serial console; VMM runs under seccomp | WSL2 KVM |
| M4 | Bare metal: VMXON | UEFI app checks and enables VMX on the BSP and logs to serial; integration test in QEMU/KVM nested with OVMF | WSL2 QEMU/KVM (L2) |
| M5 | Bare metal: first guest | VMLAUNCH a guest that executes CPUID and HLT; exits handled; EPT in place | same |
| M6 | Shared interface | Hosted and bare-metal backends implement one `Vcpu`/`GuestMemory`/exit-dispatch trait set, tested with a mock backend | — |
