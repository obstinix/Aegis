# Security Analysis and Threat Model (Phase 2)

- **Observed:** 2026-10-26. Aegis has no code, so this document defines requirements and a threat model for the staged design in [`repository-comparison.md`](repository-comparison.md) §5. Each requirement gets a regression test when its code lands.
- **Inputs:** source reading of the candidates (§4 of the landscape report); the *section structure* of `shogunlab/awesome-hyper-v-exploitation`. **The individual talks and papers it links were not read this session**, so no specific CVE is cited here.
- **Rule:** security experiments run only against Aegis itself, inside nested VMs on the owner's machine. No exploit demonstrations against third-party systems or physical hardware.

---

## 1. Assets and trust boundaries

| Asset | Must be protected from |
|---|---|
| Host kernel and host memory | Any guest; a compromised Aegis VMM process (hosted) |
| Aegis hypervisor memory, code and control structures (VMCS/VMCB, EPT/NPT tables) | Guests (bare metal); DMA from devices |
| One guest's memory and state | Other guests; the host only to the extent the design says so |
| Host files, network, credentials | The VMM process (hosted): least privilege |
| Build and release artifacts | Supply-chain tampering |

```
Hosted (stage 1)                          Bare metal (stage 2)
┌──────────────┐                          ┌──────────────┐
│ Guest (L1/L2)│  ← untrusted             │ Guest        │  ← untrusted
├──── KVM ─────┤  ← hardware boundary     ├─ VMX/SVM+EPT ┤  ← hardware boundary
│ aegis VMM    │  ← unprivileged, seccomp │ Aegis core   │  ← TCB
│ process      │                          │ (VMX root)   │
├──────────────┤                          ├──────────────┤
│ Linux kernel │  ← TCB (KVM)             │ UEFI fw / HW │  ← TCB
└──────────────┘                          └──────────────┘
```

**Attacker model:** a guest kernel with full control of its own state. It can execute any instruction, program any emulated device with arbitrary values, race its vCPUs, and modify guest memory while the hypervisor reads it. Secondary attackers: a malicious config file or CLI argument, and a compromised dependency.

**Out of scope for the first release (stated as limitations, not hidden):** physical attackers; malicious firmware or SMM; side channels beyond the host's default mitigations; device passthrough (not supported; see §3.5).

## 2. Attack surface, ranked

| # | Surface | Hosted (KVM) | Bare metal | Primary risk |
|---|---|---|---|---|
| 1 | **Device emulation** (PIO/MMIO, virtio) | In the Aegis VMM process | In the Aegis core | Memory corruption from guest-controlled values. The largest surface in production hypervisors, and the reason Firecracker and crosvm keep device sets minimal and sandboxed. |
| 2 | **VM-exit handlers and instruction decoding** | Mostly in KVM; Aegis handles `KVM_EXIT_IO`/`MMIO`/`HLT`/`SHUTDOWN` | All in Aegis | Incorrect decode, missing exit-reason handling, wrong RIP advance |
| 3 | **Guest-memory access by the hypervisor** | `vm-memory` accessors | Direct | Double-fetch / TOCTOU (guest changes data after validation); out-of-bounds GPA |
| 4 | **Second-level translation (EPT/NPT)** | KVM-owned | Aegis-owned | Mapping hypervisor memory into a guest; stale TLB after unmap (missing INVEPT); writable+executable confusion |
| 5 | **MSR/CR/I/O intercept policy** | KVM defaults + `KVM_SET_MSRS` | Aegis bitmaps | Pass-through of MSRs that control host state (e.g. `IA32_FEATURE_CONTROL`, SMRR, `EFER` misuse) |
| 6 | **Interrupt and event injection** | KVM irqchip | Aegis | Injecting an event with an invalid vector or type, which causes VM-entry failure loops or host lockup |
| 7 | **Hypercall interface** (if added) | Aegis | Aegis | Argument validation; privilege confusion |
| 8 | **Management interfaces** (CLI, config, any future socket) | Aegis | Aegis | Path traversal, unbounded sizes, injection into spawned processes |
| 9 | **Resource exhaustion** | memory, vCPUs, fds, log volume, exit storms | memory, exit storms | Host DoS |
| 10 | **Supply chain** | crates, toolchain | crates, toolchain, firmware images | Malicious or vulnerable dependency |

## 3. Requirements (safe defaults)

### 3.1 Hosted VMM process
- Runs **unprivileged**. Access to `/dev/kvm` comes through group membership only, never root. (This is exactly why the WSL user currently can't open `/dev/kvm`: correct behavior that the owner must grant deliberately.)
- After setup, applies a **seccomp allowlist** (`seccompiler`) covering only the syscalls the run loop needs. A test asserts that a forbidden syscall kills the process.
  - *Status (M3):* implemented in `aegis-kvm::seccomp` and installed by `aegis run` immediately before the run loop. Mismatch kills the process. `ioctl` is limited to `KVM_RUN`, `write` to fds 1 and 2, and `fcntl` to `F_GETFD`, which Rust std's debug builds issue when dropping fds (found with gdb after the filter killed a debug build at teardown). Verified: `aegis selftest seccomp` dies by `SIGSYS`; a full Linux boot runs under the filter; a mutant filter without `KVM_RUN` is killed on the first `KVM_RUN`.
  - *Status (M4h):* the filter is installed per thread: by every vCPU thread before its first `KVM_RUN`, and by the coordinating thread once it has every vCPU's thread ID. It never has to cover setup. Three additions, each with its argument pinned where possible:
    - `ioctl` `KVM_IRQ_LINE`, for the virtio-console's interrupt. Like `KVM_RUN`, this only acts on the VM's own fd.
    - `tgkill`, only with `tgid` = this process and signal `SIGRTMIN`: the coordinator's kick that makes a sibling vCPU's `KVM_RUN` return `EINTR` when the guest stops. The kick is a raw `tgkill` and not `pthread_kill`, because glibc's `pthread_kill` also calls `getpid`; the first two-vCPU boot was killed on that `getpid` (`syscall=39` in the audit log), and allowing a syscall only because a C library happens to issue it was rejected. No `clone` is allowed, so a thread ID cannot be reused by a new thread while the filter is on.
    - `clock_nanosleep` and `nanosleep`, for the coordinator's 10 ms kick interval.
  - *Device input (M4h):* the virtio-mmio registers and the split virtqueue are guest-controlled. Every ring index, descriptor address, length and chain is validated with checked arithmetic against the guest memory map. Chains are bounded by the queue size and by 1 MiB, indirect descriptors are rejected, and output is capped at 64 KiB. A malformed queue sets `DEVICE_NEEDS_RESET` and stops processing; it never panics the host. All of this is unit-tested with malformed inputs, not fuzzed (see §5).
- No network, no filesystem writes outside an explicitly named run directory. Later: optional namespace/jailer, as in Firecracker.
- Minimal default device set: **one serial port**. Everything else is opt-in.

### 3.2 Bare-metal core
- **Deny by default:** MSR bitmap, I/O bitmap and CR masks intercept everything not explicitly allowed. Exception bitmap is defined per policy.
- Hypervisor memory is **never mapped** into any guest EPT/NPT. A test walks the EPT after construction and asserts no hypervisor page is reachable.
- Every EPT/NPT unmap or permission reduction is followed by **INVEPT/INVVPID (or the NPT TLB-flush equivalent)**. Tested by checking the flush is called on each such path.
- **Every exit reason is handled explicitly.** An unknown exit reason shuts down *that guest* with diagnostics. It never reenters with undefined state, and never panics the whole host when avoidable.
- Fail closed on VM-entry failure: log the VM-instruction error field and stop the vCPU.

### 3.3 Guest memory and input handling
- All guest-provided addresses and lengths are validated against the VM's memory map with checked arithmetic (no wrapping).
- Data needed for a decision is **copied once** into hypervisor memory, then validated, then used. No double fetch.
- Device-register writes are bounded and type-checked. Queue indices are taken modulo the negotiated size.

### 3.4 Resources
- Per-VM caps: memory, vCPUs, log rate. Defaults are small; raising them is explicit.
- Exit-storm detection: if the exit rate on one vCPU stays above a threshold, log a warning and optionally pause the VM.
- Guest serial output to the host log is rate-limited, and control characters are escaped, so a guest can't fill the disk or inject terminal escape sequences.

### 3.5 DMA and passthrough
- **No device passthrough** until IOMMU (VT-d / AMD-Vi) enforcement is implemented and tested. Without an IOMMU, a passthrough device can DMA into hypervisor or other-guest memory, which defeats EPT/NPT entirely. Jailhouse's model, ideas only (GPL-2.0), is the reference when this milestone comes.

### 3.6 Side channels
- Rely on host kernel mitigations for the hosted path. `aegis doctor` **reports** the host's `/sys/devices/system/cpu/vulnerabilities/*` status, and never claims protection Aegis doesn't provide.

### 3.7 CLI and config
- TOML config parsed into strict types (unknown fields rejected), with explicit size limits. Paths are canonicalized and must sit under allowed directories. No shell invocation.

### 3.8 Logging and diagnostics
- Structured logs (JSON lines). Guest memory contents are never logged by default. Register dumps on fatal guest errors only. Logs contain no host secrets or environment variables.

## 4. Supply chain and build integrity
- `Cargo.lock` committed; Rust toolchain pinned with `rust-toolchain.toml`.
- `cargo deny check` (licenses + advisories + bans) and `cargo audit` in CI. Both need owner approval to install locally.
- Minimal dependencies; each new crate is justified in its commit message.
- Reproducible builds as a later goal: record toolchain version and build flags; compare artifact hashes across two clean builds.
- Release artifacts: checksums at minimum; signing once release infrastructure exists.

## 5. Security testing plan

| Test type | Target | When |
|---|---|---|
| Unit tests with adversarial inputs | GPA validation, decoders, config parser | From M0 |
| Property tests (`proptest`) | Memory map overlap and bounds; EPT builder never maps hypervisor ranges | M2 / M5 |
| Fuzzing (`cargo fuzz`) | Exit-dispatch with arbitrary exit info and register state against a mock backend; device register handlers; config parser | From M2 |
| Seccomp regression | Forbidden syscall → kill | M3 |
| Nested integration tests | Malicious test guests: wild MMIO, I/O to unassigned ports, writes to unmapped GPAs, huge exit storms | M3 / M5 |
| Code review | `security-review` skill (available in the current session); later `cloudflare/security-audit-skill` (needs approval) | Each milestone |

## 6. Vulnerability response
- Add `SECURITY.md` with a private reporting channel (GitHub private vulnerability reporting, once enabled by the owner) and a statement of supported versions. Until a release exists: "no supported versions; research software."
- Fixes land with a regression test reproducing the issue.

## 7. Lessons taken from the references
- **barevisor**: its README is explicit that it provides no protection from the guest or DMA. That's a useful warning. "Educational hypervisor" structure does **not** come with an isolation model, and Aegis must design its own.
- **Firecracker / crosvm** (documentation-level): minimal device sets and VMM sandboxing are the main levers on surface #1.
- **Jailhouse** (ideas only): static resource partitioning plus IOMMU makes isolation tractable. Aegis should prefer static assignment over dynamic sharing in early milestones.
- **awesome-hyper-v-exploitation** (structure only): its sections single out synthetic devices, hypercalls and fuzzing as the research focus in a production hypervisor, which matches surfaces #1, #2 and #7 here. Read the linked material before writing the device and hypercall milestones.
