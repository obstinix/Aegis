# Aegis Development Log

Honest record of how work was done, including AI assistance. Commit metadata stays authored solely by `obstinix` (see `docs/AEGIS-DIRECTIVE.md` §0). Disclosure lives here.

## 2026-10-26: Phase 1 discovery

- **Assistance:** Claude Code (model `claude-opus-5-5`, desktop app) ran the discovery under the owner's directive.
- **Done:**
  - Cloned `obstinix/Aegis` (HEAD `a424ee6`) and inspected the PyxisOS and PyxForge zip extracts.
  - Fetched metadata for all 570 `topic:hypervisor` repos.
  - Shallow-cloned 17 repos into a session scratch directory outside the repo and read their source. Nothing third-party was built or executed.
  - Checked host virtualization capability.
- **Written:** `research/hypervisor-landscape.md`, `research/data/hypervisor-topic-2026-10-26.md`, `research/tooling-and-skills.md`, `research/references.md`, `.gitignore`, this log. The directive itself was not transcribed; the owner should save the original text as `docs/AEGIS-DIRECTIVE.md`.
- **Commits:** Phase 1 was committed locally in three focused commits (`.gitignore`, research reports, this log). See `git log`.
- **Not done:** no pushes, no installs.
- **Security note (Phase 1):** the owner pasted a GitHub token into the chat. It was not used or stored, and revocation was recommended.

## 2026-10-26: Phase 2 comparative analysis

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session as Phase 1.
- **Done:**
  - Audited license files and headers, dependencies, and test/CI files for the shortlisted repos.
  - Looked up crates.io licenses for the candidate dependencies.
  - Checked WSL2 KVM parameters and exposed VMX features, plus QEMU accelerators and versions.
  - Scored 13 candidates against two gates and seven weighted criteria. A script recomputed the arithmetic.
- **Written:** `research/repository-comparison.md`, `research/licensing-review.md`, `research/compatibility-matrix.md`, `research/security-analysis.md`. Updated one line in `research/hypervisor-landscape.md`.
- **Key findings:**
  - Recommended direction is staged: rust-vmm on KVM first, with a clean-room bare-metal VMX core developed alongside.
  - The WSL user can't open `/dev/kvm`. This needs an owner action.
- **Not done:** no pushes, no installs, no architecture decision (that's Phase 3).

## 2026-10-26: Phase 3 architecture decision

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session.
- **Owner direction:** proceed with the staged recommendation. This is recorded as ADR-0001 Accepted.
- **Written:** `research/architecture-decision-record.md` (ADR-0001 to ADR-0012) and `research/development-roadmap.md`. These cover tracks H, B, W and integration, plus releases and the owner actions that unblock them.
- **README:** aligned with the accepted ADRs:
  - staged identity;
  - the decided design table, replacing the open decisions;
  - stable Rust;
  - the crate layout;
  - the WSL `kvm` group note;
  - two new status rows, both still "Not started".
- **Proposed, awaiting the owner:** ADR-0011 (add a `LICENSE` file) and ADR-0012 (canonical home relative to `PyxisOS/native/aegis`).
- **Process note:** the five Phase 1 and Phase 2 commits didn't use the Conventional Commits format that the README's Contributing section asks for. They are left as-is, since rewriting history needs owner authorization. Phase 3 commits onward use it.
- **Not done:** no code, no pushes, no installs.

## 2026-10-26: Phase 4, M0 (Windows)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Code was written by the assistant from the ADRs, with no third-party code consulted or copied.
- **Written:** Cargo workspace and `crates/aegis-core`:
  - `lifecycle.rs`: state machine. Tests check all 49 state/event pairs against an explicit table.
  - `memory.rs`: guest memory map with alignment, overflow, overlap, cap, bounds and permission checks.
  - `exit.rs`: VM-exit model with access-size validation and write-value masking.
  - The crate is `no_std`, `forbid(unsafe_code)`, and has 22 unit tests.
- **Bug found by a test:** `MmioAccess::new` rejected a valid 8-byte access ending at `u64::MAX`, because it checked the exclusive end (2^64). Fixed to check the inclusive last byte. The test was kept as written.
- **Verified on Windows:** fmt, clippy `-D warnings` (pedantic + `arithmetic_side_effects`), tests (22/22), docs `-D warnings`, `no_std` build for `x86_64-unknown-none`.
- **Not done (owner approval needed):** `x86_64-unknown-uefi` target install, `rust-toolchain.toml` pin, `cargo-deny`, Rust in WSL2. M0 stays "in progress".
- **Update (owner-approved):** ran `rustup target add x86_64-unknown-uefi` on `stable-x86_64-pc-windows-gnu`. `aegis-core` now builds (debug + release) and passes clippy `-D warnings` for `x86_64-unknown-uefi`, and the 22 tests still pass. Remaining for M0: toolchain pin, `cargo-deny`, Rust in WSL2.
- **Update (owner-approved):** added `rust-toolchain.toml` pinning `1.97.0` (clippy, rustfmt, UEFI + none targets). rustup downloaded `1.97.0-x86_64-pc-windows-msvc`. **Unrequested side effect:** `rustup toolchain install` also self-updated rustup from 1.29.1 to 1.29.2 (its default behavior). Future installs will pass `--no-self-update`. All M0 checks were rerun from a clean build on the pinned toolchain and passed: fmt; tests 22/22; clippy `-D warnings` on host, none and uefi; docs `-D warnings`; debug and release builds for both `no_std` targets. Remaining for M0: `cargo-deny`, Rust in WSL2.
- **Update (owner-approved):** installed `cargo-deny` 0.20.2 with `cargo install --locked`. The first background build was stopped before it finished, and nothing was installed; the foreground rerun completed in 2m 18s. Added `deny.toml`: ADR-0011 allowlist, deny yanked crates, deny wildcards, crates.io as the only source. `cargo deny check` passes. Negative test in a scratch copy: a `GPL-3.0-only` workspace license is rejected (exit 4). Only M0 item left: run the checks in WSL2 (needs Rust installed there).
- **Update (owner-approved): Rust in WSL2, M0 done.**
  - **Download:** the official `rustup-init` binary and its `.sha256` from `static.rust-lang.org`, via `curl --proto =https --tlsv1.2` (no `curl | sh`). Checksum verified. The checksum comes from the same server, so it guards against corruption, not against a compromised server.
  - **Install:** `rustup-init -y --profile minimal --default-toolchain none`. The PATH line went into `~/.profile` and `~/.bashrc`. Then `rustup toolchain install --no-self-update` in the repo installed the pinned `1.97.0-x86_64-unknown-linux-gnu`.
  - **Unexpected output, investigated:** rustup-init printed `cargo:rerun-if-env-changed=CFLAGS…` lines. Its embedded `cc` crate (strings found in the binary) printed them while probing for a C compiler. A filesystem check found no build artifacts and only the expected rustup files changed. Benign.
  - **`cargo-deny`:** 0.20.2 installed in WSL with `cargo install --locked`.
  - **Results:** all M0 checks pass in WSL2.
  - **Cleanup:** the temporary download directory was removed. The build caches `~/cargo-deny-build` (329 MB) and `~/aegis-target` were left in place.
  - **Note:** WSL rustup has no default toolchain by design. Outside the repo, use `cargo +1.97.0`.
  - **Next:** M1 (`aegis doctor`). M2 and later stay blocked on `kvm` group membership.

## 2026-10-26: Phase 4, M1 (`aegis doctor`)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Code was written by the assistant from the ADRs and the Intel SDM / AMD APM bit definitions, with no third-party code consulted or copied.
- **Added:**
  - `crates/aegis-probe`:
    - CPUID decoding, `no_std`, with no `unsafe` (`__cpuid` is a safe function on 1.97.0, verified by compiling under `forbid(unsafe_code)`);
    - Linux `/proc` and `/sys` parsers with a configurable root;
    - a live `/dev/kvm` probe;
    - a pure `Report::evaluate`.
  - `crates/aegis-cli`: the `aegis` binary (`doctor`, `doctor --json`, `doctor --raw`, `version`, `help`; exit 2 on usage errors).
  - Fixtures captured from this workstation (WSL2 and Windows), with `.gitattributes` keeping them byte-exact.
- **Tests:** 25 new (15 probe unit, 8 fixture, 2 CLI), 47 in total. Fixture assertions compare against independently observed facts (`/proc/cpuinfo` text, module parameter files, Phase 1 WMI values), not against the probe's own earlier output. A mutation check in a scratch copy (fixture `nested=N`) failed exactly the 3 dependent tests.
- **Issues found and fixed before commit:**
  - clippy `doc_markdown` on hex leaf numbers in docs;
  - one redundant closure;
  - a doc comment that overclaimed which `/dev/kvm` paths had run live;
  - a code comment that cited an unverified Rust version;
  - awkward verdict wording.
- **Design change:** ADR-0003 said `aegis-probe` would use a `linux` feature. It now uses a default `std` feature with the Linux-only parts gated by `target_os`, so the parsers are tested on Windows as well. The ADR text is updated.
- **Not done:** live `KVM_GET_API_VERSION` / `KVM_CHECK_EXTENSION`. Implemented, but they need `/dev/kvm` access (owner: `kvm` group).
- **Update: M1 done.**
  - **The `kvm` group change took four attempts by the owner.**
    - Attempts 1–3 left no trace in Ubuntu: `/etc/group` unchanged since 2026-10-09, the backup `/etc/group-` from 2026-08-26, nothing in `auth.log`. The Terminal panel showed only an empty prompt.
    - From the assistant's side, `wsl -d Ubuntu -u root` was confirmed to reach the right distro, using read-only commands only.
    - The fourth attempt worked: `kvm:x:993:obsti`, with `/etc/group` rewritten at 04:57:04 UTC.
    - The assistant didn't run `usermod` itself; it's a security setting.
  - **First live run of the `Opened` path:** API version 12; `irqchip`, `user_memory`, `pit2`, `ext_cpuid`, `immediate_exit` all present. Raw `fcntl.ioctl` calls from Python (no `kvm-ioctls`, no Aegis code) gave identical values. Both verdicts are READY.
  - **New fixture** `wsl2-ultra9-275hx-kvm` with 3 tests, 50 tests in total.
  - **A first draft of one test was wrong.** It compared whole CPUID snapshots between the two WSL2 captures, but leaf 1 EBX[31:24] is the initial APIC ID of whichever logical CPU ran CPUID (`0x13` vs `0x0f`). The assertion now compares vendor, brand, signature, feature flags and hypervisor leaf, with a comment explaining why.
  - **Checks:** all pass on Windows and WSL2.
  - **Status docs:** stale "blocked by `kvm` group" entries are updated in the README, roadmap (M1 done; M2, M3 and B0 now just "not started"), compatibility matrix, ADR-0004, the landscape report and `kvm.rs`. Earlier devlog entries are left as history.

## 2026-10-26: Phase 4, M2 (partial: backend-neutral half)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Code was written by the assistant from the ADRs and the 16550 register layout, with no third-party code copied. The `kvm-ioctls` 0.25.1 source was read to confirm its exit variants and the safety contract of `set_user_memory_region`.
- **Added:**
  - In `aegis-core`: the `Vcpu`/`PortIo` backend interface, the `PortBus` port bus with the unassigned-port policy, the `run_vcpu` dispatch loop with exit limits and a bounded trace, and a scripted mock backend (feature `mock`).
  - `aegis-devices`, with a transmit-only, output-capped 16550 UART.
  - Three NASM test guests in `tests/guests/`.
  - 34 new tests, 68 in total.
- **Issues found during development:**
  - A trait-object lifetime error in `PortBus::route`, fixed by naming `dyn PortDevice + 'd`.
  - A bus test lent one device twice; each attempt now gets its own device, because a lent device stays borrowed for the bus's lifetime.
  - Clippy `collapsible_if`, fixed with an edition-2024 `let` chain.
  - `io_exit`, a panicking test helper, was moved out of the public backend API into the mock module.
- **Design note:** port I/O is served inside `Vcpu::run`. ADR-0007 now has an "as implemented" note explaining why.
- **Paused by owner:** before `aegis-kvm` existed, the owner told me to stop, commit and push. A local `aegis-kvm` scaffold (a manifest and a one-line placeholder) and its workspace dependencies (`kvm-bindings`, `vm-memory` 0.18.0) were backed out uncommitted. The scaffold was moved to the session scratch directory, not deleted.
- **Not done:** the KVM backend; running the NASM guests; the KVM acceptance tests.

## 2026-10-26: Phase 4, M2 completed (KVM backend)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Resumed on the owner's "continue" after the M2 partial push.
- **Incident, WSL Cargo cache:** six downloaded crate archives (`vm-memory` 0.18.0, `thiserror`, `thiserror-impl`, three `winapi` crates), all fetched at 05:09 UTC, were 0 bytes, and their extracted sources were empty.
  - **Likely cause** (not proven): the WSL VM stopped before ext4 flushed delayed writes. The journal shows unclean shutdowns.
  - **Scope:** the other 159 archives and the WSL toolchain (230 files) were intact.
  - **Fix:** only those six cache entries were removed and re-downloaded, followed by `sync`; their SHA-256 sums match `Cargo.lock`.
  - **Impact:** none. Empty sources fail at compile time, so nothing was built from bad sources.
- **Added `crates/aegis-kvm`:**
  - `KvmVm` (VM plus `vm-memory` guest memory from a `GuestMemoryMap`) and `KvmVcpu<'vm>` (real-mode entry; `Vcpu` implementation mapping KVM exits to `VmExit`).
  - One `unsafe` block, with a SAFETY argument based on drop order and the vCPU borrowing the VM.
  - 2 unit tests and 5 KVM acceptance tests that assemble and run the three NASM guests.
- **First live guest run under KVM:** all acceptance tests passed in WSL2. Mutation checks confirmed that the guest-message assertion and the fd-leak check can fail.
- **Issues found:**
  - In 0.18, `vm-memory`'s region iteration lives on trait `GuestMemoryBackend`, not `GuestMemory`.
  - Clippy `items_after_statements` in a test.
  - Broken intra-doc links on Windows, where the crate is empty but its docs still render.
  - A safety check blocked a scratch-directory `rm -rf` through a shell variable; I worked around it with a fresh directory and nothing was deleted. The old scratch directories `kvm-mutant-*` and `wt-target` are still in the session scratchpad, for the owner to remove if wanted.
- **Not done:** a live unknown-exit test (mock only); M3 (Linux boot). No push yet; awaiting owner approval.

## 2026-10-26: Phase 4, M3 (Linux boot under KVM)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Boot-protocol code was written from `Documentation/arch/x86/boot.rst`, with no third-party code copied. The `seccompiler` 0.5 and `linux-loader` 0.14 sources were read to confirm their APIs.
- **Kernel:** the WSL2 kernel already on the machine (`C:\Program Files\WSL\tools\kernel`, 6.18.33.2). Its `/proc/config.gz` confirmed 8250 console, initramfs and KVM guest support, so nothing was downloaded.
- **Design decisions:**
  - Dropped `linux-loader` in favor of a byte-level zero page, with no `unsafe` (ADR-0004 note).
  - GDT selectors follow the protocol's `__BOOT_CS` 0x10 / `__BOOT_DS` 0x18. My first draft used Firecracker's 0x08/0x10; I corrected it before running.
  - The boot runs as a real `aegis run` process, so it can be timed out and sandboxed.
  - `/init` writes to `/dev/kmsg`, because the UART raises no interrupts.
- **Seccomp finding:** the first boot was killed by the filter at teardown, on `syscall=72` (`fcntl`) from the audit log.
  - A scratch copy with a log-only filter showed `fcntl` was the only violation, and that Linux otherwise booted fully.
  - gdb `catch syscall fcntl` traced all three calls to `fcntl(fd, F_GETFD)` in Rust std's `debug_assert_fd_is_open` when dropping the VM, vCPU and `/dev/kvm` fds.
  - Fix: allow `fcntl` only with `F_GETFD`.
- **Result:** Linux boots to `/init` and prints `AEGIS_M3_INIT_OK` through the UART under the enforcing filter, in about 2.5–3 s. Mutation checks (changed marker; filter without `KVM_RUN`) fail as expected.
- **Other fixes:**
  - Several clippy findings (`ExtINT` backticks, a useless conversion, `format_push_string`, `cmp_owned`, test-helper arithmetic, `Duration::from_mins`).
  - A test closure returning a borrow of a local.
  - Two scripted edits that failed on quoting were redone with exact-match edits.
  - Apparent escaped bytes in the boot log were a `grep` quoting artifact; a Python scan found none.
- **Not done:** UART interrupts, streamed console output, SMP, virtio (M4h). No push yet.

## 2026-10-26: Phase 4, M4h (SMP and virtio-console under KVM)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session, which ran out of context once and continued from a summary.
  - Device code was written from the virtio 1.2 specification (§2.7 split virtqueues, §4.2.2 virtio-mmio), and the ACPI layout from the ACPI 6.x table definitions.
  - No third-party code was copied. The `acpi_tables` 0.2.1 source was read to confirm its builder APIs and integer AML encodings.
  - The DSDT device shape was compared with QEMU microvm's description of its virtio-mmio devices.
- **Scope change (directed by the owner's follow-up, recorded in the roadmap's M4h scope note):**
  - virtio-console instead of virtio-blk, and ACPI instead of MP tables. Both follow from the guest kernel's config: `VIRTIO_BLK=m`, no `X86_MPPARSE`, and no virtio-mmio command-line devices.
  - Fuzzing is deferred, because `cargo-fuzz` needs nightly.
- **Dependencies added:**
  - `acpi_tables` 0.2.1 (Apache-2.0, no `unsafe`), with `zerocopy`/`zerocopy-derive` 0.8.63 and build-only `syn` 2.
  - `vmm-sys-util` 0.15.0, now a direct dependency (it was already transitive).
  - `cargo deny` passes. The new `syn` duplicate is a warning, documented in the licensing review.
- **Bugs found by the live runs, all fixed:**
  - **The first two-vCPU boot died by `SIGSYS`.**
    - The audit record (`type=1326 … syscall=39`) named `getpid`, which glibc's `pthread_kill` calls internally.
    - Fix: the kick is now a raw `tgkill` with the PID and thread IDs read before any filter is installed, so it is exactly the syscall the filter allows. Allowing `getpid` was rejected.
  - **vCPU 1 then failed with `KVM_RUN: EAGAIN` after 0 exits.**
    - KVM returns `EAGAIN` when INIT/SIPI changes a waiting AP's state.
    - The backend now runs `KVM_RUN` again on `EAGAIN`. That cannot spin, and a pending kick still yields `EINTR`.
  - **The M3 boot test failed after the summary format changed.**
    - Its assertion `guest stopped: Shutdown` broke.
    - The headline now names the decisive stop reason again, so the M3 test is unchanged.
  - **Review of my own docs found `_UID 0` on every DSDT device**, which breaks ACPI's uniqueness rule once there is more than one virtio window.
    - Each device now uses its index as `_UID`, with a three-device unit test.
    - My earlier note that `acpi_tables` has no integer AML encoding was wrong: `u8` implements `Aml`.
- **What the guest showed:**
  - Linux found every table where it was placed, used the MADT for SMP, and brought up 2, 4 and 8 CPUs.
  - It loaded the DSDT and bound `virtio0` on IOAPIC pin 5, level-triggered.
  - It wrote 1166 bytes through hvc0, and counted the used-buffer interrupts in its own `/proc/interrupts`.
- **Checksum evidence was tested, not assumed.**
  - The default boot does not verify checksums early, so I booted once with `acpi_force_table_verification`: no complaint.
  - A scratch mutant with a corrupted MADT checksum showed that the kernel warns ("ACPI BIOS Warning (bug): Incorrect checksum") but still uses the table, and the live test still passed.
  - The live test now asserts that no ACPI warning or error appears. With that, the mutant fails.
- **Other mutation checks**, all on scratch copies that are not committed:
  - with no used-buffer interrupt, the interrupt-count assertion fails;
  - with no kick, the VMM hangs (still running at a 30 s limit).
- **Found while reviewing the staged diff before committing:**
  - Writing `QueueReady = 1` to a queue that was already ready rewound its ring indices, so the device would replay buffers it had already returned. That only affects the guest's own data, and Linux never does it, but re-enabling a ready queue is now ignored. The test extension fails on a mutant without the guard.
  - Two doc comments were stale: `_UID = 0`, and "devices implement `PortDevice`".
- **Found by testing the code commit on its own:** I exported the code commit alone with `git archive` and ran the full suite on it.
  - The old config-error test used `vcpus = 4` as its unknown key. That key is now valid, so the test failed at that commit.
  - The one-line test fix (`balloon = true`) was folded into the unpushed code commit, which then passed 125/125 on its own.
  - While preparing that amend, a scripted step copied HEAD's test file over the working copy, because a heredoc line break ended the `&&` chain. It was restored from a backup made just before. Nothing was staged or committed in between.
- **Also fixed:**
  - clippy `too_many_lines` in `run_vm`, which is now split into helpers;
  - literal newlines that a scripted edit had put into summary strings;
  - an unused import.
- **Checks:** all pass on Windows and WSL2 (counts in the roadmap's M4h detail).
- **Not done:** virtio-console input; streaming output; virtio-blk (the kernel builds it as a module); fuzzing; B0. No push yet.

## 2026-10-26: Phase 4, M6h (virtio-console input)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session.
  - Written from the virtio 1.2 specification (§5.3 console, §2.7 split virtqueues). No third-party code was copied.
  - Linux's `drivers/char/virtio_console.c` (mainline, GPL-2.0) was read only to understand a guest behavior. Nothing was taken from it.
- **Baseline checked first:** `main` = `origin/main` = `e9c5a3d`, clean tree, no stashes. The M4h claims were verified in this session at that commit.
- **Design:**
  - **Device:** a bounded input FIFO in `VirtioConsole` (`no_std`); the receive queue is filled only when there is input.
  - **Transport:** `process()` for host-initiated work.
  - **Host:** a stdin worker in `aegis-kvm::input` (`poll` + `read`, backpressure) under its own seccomp filter; the device sits behind its own mutex.
  - **Config:** `vm.toml` gains `virtio_console_stdin`, off by default.
  - No new dependencies.
- **Live finding: the guest dropped input.**
  - The first live run hung with the guest blocked in `read()`. A 10 s `poll` timeout in the test `/init` turned the hang into a diagnosis: "32 bytes in, 0 pending", meaning the device had written every byte into guest buffers, yet userspace saw none.
  - Reading the driver showed why. Without multiport, `add_port()` posts receive buffers, kicking after each one, before it lists the port, and `in_intr()` frees any buffer that completes for a port it cannot find. The device had answered the first kick.
  - Fix: the receive path opens on the guest's first transmit, and the receive queue is processed in that same exit.
  - A mutant without the gate reproduces the original failure exactly.
  - This is a documented deviation: device behavior shaped by one guest driver. The protocol-level fix (multiport) is not implemented.
- **Security review findings, both fixed:**
  - **DMA permissions:** device memory access ignored the memory map's read/write permissions, because it went through the image-loading path. Writes now need write permission and reads need read permission. A live test with an RWX region and an RX region covers it, and a mutant without the check fails that test.
  - **Shutdown:** a `read` could stay blocked if another process drained the shared stdin between `poll` and `read`. The worker now registers as an SMP `HostThread` and is kicked out like a vCPU. A unit test kicks a thread out of a blocked `read`.
- **Doc repair:**
  - My M4h edit of ADR-0008 had dropped the `## ADR-0009` heading line in the published `e9c5a3d`, so ADR-0009 read as part of ADR-0008. The heading is restored, and the text below it is byte-identical to `5a78586`. A check of every document M4h touched found no other lost heading.
  - The `aegis-kvm` crate docs still said there was one `unsafe` operation and that no device serves MMIO; both are corrected.
- **Other fixes:**
  - a test assertion that a `sed` edit split with a literal newline;
  - a test of the worker filter that checked a copy of the rule set instead of the real one, which now has an `input_worker_rules()` function to test;
  - a clippy semicolon.
- **Results:** see the roadmap's M6h detail for counts.
  - Windows: 111/111.
  - WSL2: 151/151 with 8 live tests (143 run in a plain `cargo test`).
  - Live checks also run by hand: immediate end of file; late input; 1/2/4/8-vCPU boots unaffected.
- **Not done:** multiport; interactive terminal use (output is not streamed); `VIRTIO_CONSOLE_F_SIZE`; fuzzing; B0 (its prerequisites are present in WSL2); M5h (hardware). No push yet; it awaits owner approval.

## 2026-10-26: Phase 4, B0 (nested test harness)

- **Assistance:** Claude Code (model `claude-opus-5-5`), same session. Started on the owner's "continue" after the M6h push.
  - The FAT writer follows Microsoft's FAT specification (fatgen103), and the UEFI calls follow UEFI 2.10. No third-party code was copied.
  - The `uefi` 0.41 and `uefi-raw` 0.17 sources were read to confirm the APIs and the types used.
- **Prerequisites checked, not assumed.**
  - Present in WSL2: QEMU 8.2.2, OVMF 2024.02 (including a combined `/usr/share/ovmf/OVMF.fd`, so `-bios` works as the criterion says), `kvm_intel nested=Y`, and the pinned UEFI target.
  - Absent: any FAT tools (`mtools`, `dosfstools`).
- **Licence finding.** ADR-0003 names the `uefi` crate, but `uefi` 0.41 always depends on `ucs2` 0.3.3, which is MPL-2.0 and outside the ADR-0011 allowlist.
  - Rather than change licence policy, `aegis-uefi` uses `uefi-raw` 0.17 (MIT OR Apache-2.0, depending only on allowed crates), with three small `unsafe` blocks.
  - Allowing `ucs2` is left as an owner decision, recorded in the ADR-0003 note and the licensing review.
- **No tool installs.** `xtask` writes the FAT16 image itself, the way M3 wrote the boot protocol instead of using `linux-loader`. It has unit tests that parse the image back.
- **Result.** `cargo xtask uefi-test` passed on its first live run, and 10/10 repeats passed.
  - OVMF loaded `BOOTX64.EFI` from the generated image, the app printed `AEGIS_B0_HELLO` (UEFI 2.70), and the VM powered itself off in about 0.6 s.
  - Three failure mutants (wrong marker, no power-off, misnamed boot file) each failed with the reason and the serial log, and left no QEMU behind.
  - In the misnamed-file run, the UEFI shell mounted the image as `FS0:`, independent evidence that the FAT16 volume is valid.
- **Reproducibility.** It wasn't reproducible at first: two builds differed in 10 bytes, even in one directory.
  - `cmp` traced the difference to lld-link's timestamp and per-build PDB debug directory. `/Brepro` alone was not enough; `/Brepro /DEBUG:NONE` gives byte-identical EFI binaries and images across target directories and rebuilds.
  - This holds on one host only (Windows and Linux builds differ), and it was checked by hand. So README Phase 0 stays unticked, with a note saying why.
- **Smaller fixes:**
  - clippy `arithmetic_side_effects` in the FAT writer, removed by restructuring rather than allow-listed;
  - an unused constant;
  - `Duration::from_mins`;
  - `assert_eq!`.
- **Checks:**
  - Windows: 123/123.
  - WSL2: 163/163 with 8 live tests (155 run in a plain `cargo test`), plus `uefi-test`.
  - fmt, clippy (including `aegis-uefi` for the UEFI target), docs, the `no_std` builds and `cargo deny` pass on both.
- **Not done:** VMX (B1); CI; PDB symbols for debugging; the `ucs2` licence decision. No push yet; it awaits owner approval.

## 2026-10-27: CI, streamed console output, M5h script, fuzzing preparation

- **Assistance:** Claude Code (model `claude-opus-5-5`, desktop app), same session. The work followed the owner's directive of 2026-10-27, which set the priorities: CI, streaming `aegis run` output, ThinkPad validation, fuzzing preparation, and the `ucs2` licence question. The details are in the roadmap's "Cross-cutting work" section.
- **Commits** (local, authored by `obstinix`, not yet pushed):
  - `21075a5` `repro-test`;
  - `1fdfe8f` CI;
  - `b2022c1` output sinks;
  - `fe34bf4` streaming;
  - `09d1f5e` the M5h script;
  - `abd1259` the licensing review;
  - `97ad0b8` the fuzz crate;
  - then these docs.
- **Installs: none.**
  - Already present and used: Windows' nightly toolchain (`nightly-x86_64-pc-windows-msvc`, rustc 1.100.0-nightly 2026-09-23), the MSVC Build Tools, including their AddressSanitizer runtime, and cargo-deny 0.20.2.
  - Downloaded, as ordinary cargo dependency fetches, into the cargo cache: the `libfuzzer-sys` dependency tree, and `uefi` 0.41 with `ucs2` 0.3.3 in a scratch project outside the repository, only to read their source.
  - cargo-fuzz was not installed, and WSL2 was not given nightly Rust or a C++ compiler; both need owner approval.
- **CI.** The workflow's own steps were run locally from the parsed YAML, on Windows and in WSL2, and all passed. It has not run on GitHub; that happens on push. README Phase 0 stays unticked until it has.
- **Streaming.**
  - The live test proves streaming without timing assumptions. It waits for the guest's prompt on stdout while `aegis` is still running, and only then sends the input that answers it.
  - One pre-existing gap was fixed on the way: a panic during a run could leave the stdin worker running and hang the scope.
  - The console is still not interactive (no raw mode, no Ctrl-C passthrough), and the docs say so.
- **Hardware validation.** The script was checked under WSL2 only. It has not been run on the ThinkPad, and no ThinkPad result is recorded anywhere.
- **Fuzzing findings:**
  - libFuzzer's coverage flags work on stable Rust, but on Windows linking needs the AddressSanitizer runtime, hence nightly.
  - On Windows a Rust abort is a fast-fail that libFuzzer cannot catch, so crash inputs were silently not saved. The targets now save the input from a panic hook. A planted bug proved it: the fuzzer found it, the saved input reproduced it on stable, and it replayed cleanly once the bug was reverted.
  - Two first-draft invariants were wrong about the device and were corrected before any fuzzing: "status has only defined bits" (the transport stores what the driver writes) and an unconditional kernel-path check (Windows `Path::join` semantics).
- **`ucs2`.** Its source was inspected (licensing review §3.1): MPL-2.0, no per-file headers, a mandatory dependency of `uefi` 0.41. The options are written up for the owner; `deny.toml` is unchanged.
- **Checks:**
  - Windows: 134/134.
  - WSL2: 176/176 including the live tests.
  - The fuzz crate passes 15 tests on both.
  - `uefi-test` and `repro-test` passed, and fmt, clippy (0 findings), docs, the `no_std`/UEFI builds and `cargo deny` pass on both.
  - One local Windows run of the fuzz CI step failed with `LNK1104`, a linker file lock. The same environmental issue was seen earlier in this project. The full job passed on rerun after a clean build of the fuzz crate.
- **Not done:** a GitHub CI run (no push yet); the ThinkPad run (M5h, owner); cargo-fuzz and Linux fuzzing (need approval); fuzzing exit dispatch; the `ucs2` decision (owner). No push yet; it awaits owner approval.
