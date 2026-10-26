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
