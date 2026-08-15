# Fork auto-sync + auto-release + auto-update

This fork stays current with `openai/codex` and ships installable builds for
**Linux amd64** and **Windows amd64** without manual work. Three moving parts:

```
 openai/codex ─(every 6h)─► GitHub Actions on its-mash/codex ─► GitHub Release
   merge latest STABLE upstream release   build both platforms:      │
                │                          Linux amd64 (.tar.gz)      │
         conflict? run summary + fail      Windows amd64 (.zip)       ▼
         (you resolve locally, push)                    your machine auto-updates:
                                             Linux  codex-update.sh  (systemd timer)
                                             Windows codex-update.ps1 (scheduled task)
```

- **Sync + build + release** run entirely in GitHub Actions
  ([`.github/workflows/fork-sync-release.yml`](../.github/workflows/fork-sync-release.yml)),
  building both `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.
- **Your local install auto-updates** from the fork's latest Release, the same
  way official codex updates from official releases — Linux via
  [`codex-update.sh`](codex-update.sh), Windows via
  [`codex-update.ps1`](codex-update.ps1).

## 1. GitHub Actions: sync + release

`fork-sync-release.yml` runs every 6 hours (and on manual dispatch). Its jobs are
`sync` → `prepare` → `build` (a Linux + Windows matrix) → `publish`:

1. **sync** — merges the **latest stable `openai/codex` release** (the tag behind
   `releases/latest`; `rust-v*-alpha.*` prereleases are ignored) into this fork's
   `main`. Upstream `main` commits without a new stable release are **not**
   synced — the fork only moves at upstream release points.
   - Upstream cuts a release as a side commit off `main` whose only change is the
     workspace **version stamp** (`version = "0.0.0"` → `"0.147.0"`). The job merges
     the commit that release was cut *from*, so the fork gets exactly the released
     code without importing a version line it does not use — and which would
     otherwise conflict on every later release. If a release commit ever carries
     real content, that commit is merged instead.
   - **Already contains the release / no new release** → nothing to do.
   - **Clean merge** → pushes `main`.
   - **Conflict** → the merge is aborted in CI, the fix steps are written to the
     run **summary**, and the run **fails** so GitHub emails you. (The fork has
     Issues disabled, so no issue is opened.) Auto-sync stays paused until you fix
     it locally.
2. **build** — for each of `x86_64-unknown-linux-gnu` (on `ubuntu-latest`) and
   `x86_64-pc-windows-msvc` (on `windows-latest`), builds `codex` and
   `codex-code-mode-host` and packages `bin/` into a `.tar.gz` (Linux) or `.zip`
   (Windows), each with a `.sha256`.
3. **publish** — attaches both platforms' archives to one GitHub Release
   (`fork-<date>-<sha>`). If one platform's build fails, the other still ships and
   the run is marked failed so you get notified.

`rerere` is enabled in CI and locally, so a conflict you resolve once is replayed
automatically the next time the same hunk conflicts.

### Manually triggering a sync (no admin needed)

`workflow_dispatch` needs repo-admin the fork token lacks, so to force an
on-demand sync between scheduled ticks, bump the trigger file with `[sync]` in
the commit message:

```bash
cd /home/benty/codex
date >> .github/sync-trigger
git commit -am "[sync] manual sync"
git push origin main
```

The `[sync]` marker makes the push run the merge step (same code path as the
scheduled cron); the trigger file is outside `paths-ignore` so the run actually
starts. The scheduled every-6h run needs none of this.

### When a sync conflict is flagged

You get a failed-run email; open the run and its **summary** shows the conflicting
files and these steps. Resolve on the machine that holds the clone:

```bash
cd /home/benty/codex               # 'upstream' remote = openai/codex
tag=$(gh release view --repo openai/codex --json tagName --jq .tagName)
git fetch --no-tags upstream "refs/tags/$tag:refs/tags/$tag"
git merge "$tag^1"                 # the release point; resolve the conflicts
git add -A && git commit           # completes the merge
git push origin main               # this push builds + publishes a release
```

The next scheduled run sees a clean tree and resumes. `GITHUB_TOKEN`'s default
`contents: write` is all the workflow needs — no PAT.

## 2. Install & use

Both platforms consume the **same GitHub Release**. You do **not** need to clone
this repo — the updater scripts are self-contained. The repo is PUBLIC, so no
`gh` login or token is needed.

### Install without cloning (recommended)

**Linux amd64** — one command installs the latest fork release and sets up
`~/.local/bin/codex`; run it again any time to update:

```bash
curl -fsSL https://raw.githubusercontent.com/its-mash/codex/main/fork-tools/codex-update.sh | bash
```

Add hourly auto-update (systemd --user timer, also no clone):

```bash
curl -fsSL https://raw.githubusercontent.com/its-mash/codex/main/fork-tools/install-updater.sh | bash
```

**Windows amd64** (PowerShell) — installs to `%LOCALAPPDATA%\codex-fork` and adds
`codex` to your PATH; re-run to update:

```powershell
$u='https://raw.githubusercontent.com/its-mash/codex/main/fork-tools/codex-update.ps1'
irm $u -OutFile "$env:TEMP\codex-update.ps1"; & "$env:TEMP\codex-update.ps1"
```

> `codex-update.sh` needs `bash`, `curl`, `tar`, `python3`; `codex-update.ps1`
> needs PowerShell 5+. Neither needs `git`, `gh`, or a clone. After install, open
> a new terminal so `codex` is on `PATH`.

Then jump to [**Use it**](#use-it). The rest of this section covers cloned-repo
and fully-manual installs.

### Linux amd64 (from a clone, or manual)

From a clone the same scripts are local:

```bash
fork-tools/install-updater.sh          # enable an hourly systemd --user timer
systemctl --user start codex-update.service   # run once now
fork-tools/codex-update.sh             # ...or update by hand any time
```

The updater downloads the `*-x86_64-unknown-linux-gnu.tar.gz` asset, extracts to
`~/.codex/packages/standalone/releases/<tag>-<triple>/bin/`, and atomically
repoints `~/.codex/packages/standalone/current`; `~/.local/bin/codex ->
current/bin/codex` follows. Uninstall the timer with
`fork-tools/install-updater.sh --uninstall`.

Fully manual (no scripts at all):

```bash
# pick the newest release from https://github.com/its-mash/codex/releases/latest
curl -fsSLO https://github.com/its-mash/codex/releases/latest/download/  # (see assets)
# ...or with gh:
gh release download --repo its-mash/codex --pattern '*-x86_64-unknown-linux-gnu.tar.gz'
tar -xzf codex-*-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 codex-*/bin/codex ~/.local/bin/codex
install -Dm755 codex-*/bin/codex-code-mode-host ~/.local/bin/codex-code-mode-host
```

<a id="use-it"></a>
Use it:

```bash
codex --version
codex                       # interactive TUI
codex exec "…"              # non-interactive
codex teammate --team-name <team> --agent-name <name>   # native team member
```

### Windows amd64 (from a clone, or manual)

From a clone (the no-clone one-liner is above):

```powershell
pwsh -File fork-tools\codex-update.ps1
```

The updater downloads the `*-x86_64-pc-windows-msvc.zip` asset, verifies its
`.sha256`, extracts to `%LOCALAPPDATA%\codex-fork\releases\<tag>-<triple>\`,
repoints the `%LOCALAPPDATA%\codex-fork\current` junction, and adds `current\bin`
to your user `PATH` (open a new terminal afterward). Run it again any time to
pick up the latest release. To update on a schedule, register a daily Scheduled
Task that fetches + runs the script (no clone needed):

```powershell
$dst = "$env:LOCALAPPDATA\codex-fork\codex-update.ps1"
$cmd = "irm https://raw.githubusercontent.com/its-mash/codex/main/fork-tools/codex-update.ps1 -OutFile `"$dst`"; & `"$dst`""
$act = New-ScheduledTaskAction -Execute (Get-Command pwsh).Source -Argument "-NoProfile -Command `"$cmd`""
$trg = New-ScheduledTaskTrigger -Daily -At 9am
Register-ScheduledTask -TaskName 'codex-fork-update' -Action $act -Trigger $trg
```

Manual install (no script): download the `*-x86_64-pc-windows-msvc.zip` from the
[Releases page](https://github.com/its-mash/codex/releases/latest), extract it,
and add its `bin` folder to your `PATH`.

Use it (any terminal after PATH update):

```powershell
codex --version
codex                       # interactive TUI
codex exec "…"
codex teammate --team-name <team> --agent-name <name>
```

### Important: don't run the built-in `codex update`

`codex update` pulls from **OpenAI's** releases and would overwrite the fork build
with upstream codex. Use this fork's updater (`codex-update.sh` /
`codex-update.ps1`) instead.

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

## One-time: silence inherited upstream CI

The fork inherits every `openai/codex` workflow (`blocking-ci`, `rust-ci`,
`postmerge-ci`, release jobs, …). They fail on the fork because they need
OpenAI's secrets/infra, and each push would otherwise email you failures. The
`gh` CLI token here is not a repo admin, so disable them once from the web UI:

> **Repo → Settings → Actions → General**, or **Actions tab → each workflow →
> ⋯ → Disable workflow.** Keep **`fork-sync-release`** enabled; disable the rest.

Disabling is a repo-state setting, so it survives future upstream merges (the
workflow files stay on disk but do not run).

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
