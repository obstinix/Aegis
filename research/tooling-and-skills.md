# Tooling and Skills Vetting (§10)

- **Observed:** 2026-10-26, from GitHub API metadata plus each repo's README. Install scripts and package contents were **not** audited line by line.
- **Installed this session:** **nothing.** Every candidate below either runs code at install time, adds hooks or MCP servers, or calls the network, and all of those sit behind the §0 confirmation gate.
- **Claude Code version:** not determinable from the shell (`claude` is not on PATH; the session runs inside the Claude desktop app).

| # | Repository | Stars / license | What it is | Install method (per README) | Network / credentials | Fit for Aegis | Status |
|---|---|---|---|---|---|---|---|
| 1 | [tigerless-labs/autoharness](https://github.com/tigerless-labs/autoharness) | 10,752 / MIT | Claude Code **plugin**: hooks plus an MCP server that distill "skills" from your sessions and write them into `.claude/skills/` | `/plugin marketplace add` + `/plugin install` | Local; needs python3 for hooks | Low. It auto-writes agent instructions into the repo, which conflicts with reviewed, deliberate project context. | **unsuitable** (for now) |
| 2 | [thedotmack/claude-mem](https://github.com/thedotmack/claude-mem) | 103,156 / Apache-2.0 | Memory **plugin**. "Captures everything your agent does"; the installer defaults to a hosted memory service with browser sign-in. | `npx claude-mem install` or marketplace | Hosted by default; local provider is opt-in | Low. Shipping session transcripts of security work to a hosted service conflicts with §0. Claude Code's built-in memory already covers the need. | **unsuitable** |
| 3 | [tashfeenahmed/freellmapi](https://github.com/tashfeenahmed/freellmapi) | 41,635 / MIT | Self-hosted **LLM router** across 34 free-tier providers; Docker | `curl … \| bash` one-liner or Windows .exe | Sends prompts and code to third-party free-tier LLM providers; stores provider keys | None. Routes source code to many third parties, and its primary install is `curl \| bash`. | **unsuitable** |
| 4 | [Panniantong/Agent-Reach](https://github.com/Panniantong/Agent-Reach) | 102,551 / MIT | Agent **web/social reader** (Twitter, Reddit, YouTube, Bilibili…), uses Chrome site cookies | Agent fetches and follows a remote `install.md` | Reads browser login cookies | None. Unrelated to hypervisor work, and it handles credentials. | **unsuitable** |
| 5 | [usestrix/strix](https://github.com/usestrix/strix) | 71,914 / Apache-2.0 | AI **pentest tool** for web apps/APIs; Docker sandbox | `curl -sSL https://strix.ai/install \| bash`; also `npx skills add` | Needs an LLM API key; pulls Docker images | Low. Targets web/OWASP, not a bare-metal or VMM attack surface. | **unsuitable** |
| 6 | [anthropics/claude-plugins-official](https://github.com/anthropics/claude-plugins-official) | 44,589 / Apache-2.0 | Official Anthropic plugin **marketplace** (directory, not one tool) | `/plugin install <name>@claude-plugins-official` | Depends on the plugin; the README warns that Anthropic does not control third-party plugin contents | Medium, as a source. Individual plugins need their own vetting. | **optional** (per plugin) |
| 7 | [morluto/rea](https://github.com/morluto/rea) | 385,462 / MIT | Reverse-engineering **MCP server** + workflow instructions; drives Ghidra/IDA/Hopper | `npx rea-agents setup` (rewrites agent config, with backups) | Optional provider config | Low to medium. Could help inspect Aegis's own ELF/EFI outputs; PyxForge already has a binary inspector. | **optional**, not now |
| 8 | [OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator) | 13,640 / Apache-2.0 | Desktop app + daemon to run multi-agent teams | Desktop installer | Telemetry includes an installation ID and **GitHub username** | Low. Multi-agent orchestration has no recorded need (§0 scope). | **unsuitable** |
| 9 | [cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill) | 47,146 / MIT | Markdown **skill** for multi-phase security audits with verified findings | `npx skills add … --skill security-audit` (project-local by default; `--global` optional) | The skill itself is instructions. The `skills` CLI fetches from GitHub. The README asks for an OS sandbox when building target code. | **High** for Phase 5 security reviews of VM-exit handlers, MMIO/PIO decoders and the CLI. | **optional**: recommended once code exists. Needs owner approval to run `npx`. |
| 10 | [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | 176,266 / MIT | Prompt **skill/plugin** for minimal code; adds a cost "band" and a local board | `/plugin marketplace add` + `/plugin install` | Local by default (per README) | Low to medium. Overlaps with existing style rules in this repo's docs. | **optional** |

## Recommendation

- **Install now:** nothing.
- **Owner decision, after Phase 4 starts:** `cloudflare/security-audit-skill`, installed project-locally with `npx skills add https://github.com/cloudflare/security-audit-skill --skill security-audit`. Before running it, review the `skills` CLI package and the files it writes. Record the verification result here.
- **Already available, no install needed:** the Claude Code session in use has `security-review` and `code-review` skills built in. That covers most of what candidates 5 and 9 offer for early phases.

## Development toolchain observed (Windows host)

`rustc`, `cargo`, `rustup` (`%USERPROFILE%\.cargo\bin`), `nasm`, `gdb` (WinLibs), `clang` (llvm-mingw), `go`, `qemu-system-x86_64` (`C:\Program Files\qemu`). Versions are not yet recorded; record them in Phase 4 with `--version`.
WSL2 Ubuntu: `qemu-system-x86_64` present; `cargo` absent.
