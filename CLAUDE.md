# its-mash/codex fork — Claude notes

This is a public fork of `openai/codex` (`origin` = its-mash/codex, `upstream` =
openai/codex). Upstream's coding conventions in `AGENTS.md` apply; this file covers
only the fork's own rules.

## Sync policy: latest stable upstream release ONLY

The fork tracks the **latest stable `openai/codex` release** (the tag behind
`releases/latest`, e.g. `rust-v0.147.0`) — **not** upstream `main`, and **not**
`rust-v*-alpha.*` prereleases. New upstream commits without a new stable release
mean **no sync**. Implemented in `.github/workflows/fork-sync-release.yml`
(`sync` job, every 6h + `[sync]`-marked pushes); details and the conflict-fix
flow are in `fork-tools/README.md`.

To sync/merge upstream manually, target the release, never `upstream/main`:

```bash
tag=$(gh release view --repo openai/codex --json tagName --jq .tagName)
git fetch --no-tags upstream "refs/tags/$tag:refs/tags/$tag"
git merge "$tag^1"   # ^1 = the release point (see below)
```

Merge `"$tag^1"`, not the tag: upstream cuts a release as a side commit off `main`
whose only change is the workspace version stamp (`version = "0.0.0"` → `"0.147.0"`).
Merging the stamp would conflict on that line at every later release (base `0.0.0`
vs ours `0.147.0` vs theirs `0.148.0`), and the fork does not use that version — it
ships `fork-<date>-<sha>` releases. Merge the tag commit itself only if a release
ever changes files other than `codex-rs/Cargo.toml`.

Never run the built-in `codex update` on this machine — it installs OpenAI's
official release and reverts the fork. The local install auto-updates from this
fork's Releases via `fork-tools/codex-update.sh` (systemd user timer).

## Fork code convention: fork-owned files + 1–3 line hooks

To keep upstream merges conflict-free, fork logic lives in **fork-owned files**
(first line `//! Fork-owned: ...`); files that also exist upstream carry only
minimal hooks (a `mod x; // fork` line, a one-line call, a serde alias, an enum
variant + match arm). When adding fork code:

- Put `impl` blocks in a child module of the upstream file (child modules can use
  the parent type's private fields); add the `mod` hook at the end of the mod list.
- Fork tests go in child test modules (`#[path = "..._tests.rs"] mod ...;` at the
  end of the upstream test file) so they reuse upstream fixtures via `use super::*`.
- Prefer wrapper types over changing upstream signatures.

Main fork-owned areas: `codex-rs/ext/automation`, `codex-rs/ext/external-team`,
`codex-rs/ext/extension-api/src/external_team.rs`, `codex-rs/cli/src/teammate.rs`,
`codex-rs/config/src/external_team.rs`,
`codex-rs/core/src/tools/handlers/multi_agents_v2/external_team.rs`,
`codex-rs/core/src/codex_thread/background_terminal_poll.rs`,
`codex-rs/app-server/src/request_processors/thread_processor/loop_automation.rs`,
`codex-rs/tui/src/{app/external_team.rs,app/loop_actions.rs,loop_command.rs,`
`chatwidget/inbound_inter_agent.rs,chatwidget/loop_slash.rs,app_server_session/loop_automation.rs}`.

## Regenerating generated artifacts (never hand-merge these)

- App-server schema `.zst` + fixtures:
  `python3 codex-rs/app-server-protocol/scripts/write_schema_fixtures.py`
  (and again with `--experimental`).
- `codex-rs/core/config.schema.json`:
  `cargo run -p codex-core --bin codex-write-config-schema`.

## Testing gotcha

Multi-agent tests in `codex-core` overflow the default test stack in debug
builds — run with `RUST_MIN_STACK=8388608` (matches upstream CI).
