# Fork auto-sync + auto-release + auto-update

This fork stays current with `openai/codex` and ships installable builds without
manual work. Three moving parts:

```
 openai/codex ──(every 6h)──► GitHub Actions on its-mash/codex ──► GitHub Release
        merge into this fork's main          build release binaries        │
                │                                                           │
         conflict? open an issue + fail                                     ▼
         (you resolve locally, push)                     your machine: codex-update.sh
                                                          swaps standalone/current
```

- **Sync + build + release** run entirely in GitHub Actions
  ([`.github/workflows/fork-sync-release.yml`](../.github/workflows/fork-sync-release.yml)).
- **Your local install auto-updates** from the fork's latest Release, the same
  way official codex updates from official releases
  ([`codex-update.sh`](codex-update.sh)).

## 1. GitHub Actions: sync + release

`fork-sync-release.yml` runs every 6 hours (and on manual dispatch):

1. **sync** — merges the latest `openai/codex` `main` into this fork's `main`.
   - **Clean merge** → pushes `main`, then the `release` job builds the
     `codex` and `codex-code-mode-host` release binaries and publishes a GitHub
     Release (`fork-<date>-<sha>`) with a `.tar.gz` for `x86_64-unknown-linux-gnu`
     plus a `.sha256`.
   - **Conflict** → the merge is aborted in CI, an issue labelled
     `upstream-sync-conflict` is opened/refreshed with the exact local commands to
     resolve it, and the run **fails** (so GitHub emails you). Auto-sync stays
     paused until you fix it.

`rerere` is enabled in CI and locally, so a conflict you resolve once is replayed
automatically the next time the same hunk conflicts.

### When a sync conflict is flagged

You get a failed-run notification and an open issue. Resolve it on this machine:

```bash
cd /home/benty/codex
git fetch upstream main          # 'upstream' remote = openai/codex
git merge upstream/main          # resolve the conflicts it reports
git add -A && git commit         # completes the merge
git push origin main             # this push builds + publishes a release
```

The next scheduled run sees a clean tree, closes the conflict issue, and resumes.

> Requires the repo secret to allow Actions to push and open issues — the default
> `GITHUB_TOKEN` already has `contents: write` + `issues: write` as declared in the
> workflow. No PAT needed.

## 2. Local auto-update

[`codex-update.sh`](codex-update.sh) polls the fork's latest Release and, if it is
newer than what's installed, downloads the tarball, extracts it to
`~/.codex/packages/standalone/releases/<tag>-<triple>/bin/`, and atomically
repoints `~/.codex/packages/standalone/current`. Your existing
`~/.local/bin/codex -> current/bin/codex` symlink follows automatically.

Install the hourly timer:

```bash
fork-tools/install-updater.sh          # enable hourly OnCalendar timer
systemctl --user start codex-update.service   # run once now
journalctl --user -u codex-update.service -f  # watch
```

Run it by hand any time:

```bash
fork-tools/codex-update.sh
```

Uninstall the timer: `fork-tools/install-updater.sh --uninstall`.

### Important: don't run the built-in `codex update`

`codex update` pulls from **OpenAI's** releases and would overwrite `current` back
to upstream codex, discarding the fork. Use `codex-update.sh` (this fork's updater)
instead. Both manage the same `current` slot, so whichever ran last wins.

### Teammate binaries

The bb-team teammate launcher prefers `codex-rs/target/dev-small/codex` if present
and otherwise falls back to `command -v codex` (= `~/.local/bin/codex`, the
auto-updated fork release). Once you rely on auto-update, remove the stale local
build so teammates use the release too:

```bash
rm -f /home/benty/codex/codex-rs/target/dev-small/codex \
      /home/benty/codex/codex-rs/target/dev-small/codex-code-mode-host
# or pin it explicitly:  export CODEX_TEAMMATE_BIN="$HOME/.local/bin/codex"
```

## Status & logs

- `~/.codex-fork-sync/update.log` — local updater log.
- `~/.codex-fork-sync/installed-tag` — the release tag currently installed.
- GitHub → Actions tab — sync/release run history.
- GitHub → Issues (`upstream-sync-conflict`) — open only while a sync is blocked.

## Configuration knobs (env)

| Variable | Default | Meaning |
| --- | --- | --- |
| `CODEX_FORK_REPO_SLUG` | `its-mash/codex` | Fork repo the updater pulls releases from |
| `CODEX_FORK_TARGET` | `x86_64-unknown-linux-gnu` | Release target triple |
| `CODEX_UPDATE_INTERVAL` | `hourly` | `OnCalendar=` for the updater timer |
| `CODEX_FORK_KEEP_RELEASES` | `3` | Old fork releases kept under `releases/` |
