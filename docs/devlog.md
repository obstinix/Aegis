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
