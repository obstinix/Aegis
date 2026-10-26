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
