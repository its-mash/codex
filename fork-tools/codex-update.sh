#!/usr/bin/env bash
# codex-update.sh — auto-update the locally installed codex to the fork's latest
# GitHub Release, the same way official codex updates from official releases.
#
# It downloads the latest release asset published by fork-sync-release.yml and
# atomically repoints ~/.codex/packages/standalone/current at it, so the existing
# ~/.local/bin/codex -> current/bin/codex symlink follows with no other changes.
#
# Safe to run from a systemd user timer or by hand: `codex-update`.
# It is idempotent: if the installed tag already matches the latest release it exits quietly.
set -uo pipefail

REPO_SLUG="${CODEX_FORK_REPO_SLUG:-its-mash/codex}"
TARGET_TRIPLE="${CODEX_FORK_TARGET:-x86_64-unknown-linux-gnu}"
STANDALONE_DIR="${CODEX_STANDALONE_DIR:-$HOME/.codex/packages/standalone}"
BIN_SYMLINK="${CODEX_BIN_SYMLINK:-$HOME/.local/bin/codex}"
STATE_DIR="${CODEX_FORK_STATE_DIR:-$HOME/.codex-fork-sync}"
KEEP_RELEASES="${CODEX_FORK_KEEP_RELEASES:-3}"

RELEASES_DIR="$STANDALONE_DIR/releases"
CURRENT_LINK="$STANDALONE_DIR/current"
INSTALLED_MARKER="$STATE_DIR/installed-tag"
LOG_FILE="$STATE_DIR/update.log"
mkdir -p "$STATE_DIR" "$RELEASES_DIR"

ts() { date -Is; }
log() { printf '%s %s\n' "$(ts)" "$*" | tee -a "$LOG_FILE" >&2; }

notify() {
  local urgency="$1" title="$2" body="$3"
  export DISPLAY="${DISPLAY:-:0}"
  if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
  fi
  command -v notify-send >/dev/null 2>&1 &&
    notify-send -u "$urgency" -a "codex-update" "$title" "$body" 2>/dev/null || true
  log "[$urgency] $title — $body"
}

fail() { notify critical "codex-update failed" "$1"; exit 1; }

# --- discover the latest release --------------------------------------------
fetch_latest() {
  # Prefer gh (works for private repos too); fall back to the public API.
  if command -v gh >/dev/null 2>&1 &&
     gh release view --repo "$REPO_SLUG" --json tagName,assets >"$STATE_DIR/.latest.json" 2>/dev/null; then
    return 0
  fi
  local api="https://api.github.com/repos/$REPO_SLUG/releases/latest"
  curl -fsSL -H "Accept: application/vnd.github+json" "$api" >"$STATE_DIR/.latest.json" 2>>"$LOG_FILE"
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar  >/dev/null 2>&1 || fail "tar is required"

if ! fetch_latest; then
  fail "could not query the latest release of $REPO_SLUG (no release yet, or auth/network issue)"
fi

latest_tag="$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1]))
print(d.get("tagName") or d.get("tag_name") or "")' "$STATE_DIR/.latest.json")"
[[ -n "$latest_tag" ]] || fail "latest release has no tag; see $STATE_DIR/.latest.json"

asset_url="$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1])); triple=sys.argv[2]
assets=d.get("assets") or []
def name(a): return a.get("name","")
def url(a):  return a.get("url") or a.get("browser_download_url") or a.get("apiUrl") or ""
cands=[a for a in assets if name(a).endswith(".tar.gz") and triple in name(a)]
print(url(cands[0]) if cands else "")' "$STATE_DIR/.latest.json" "$TARGET_TRIPLE")"
asset_name="$(basename "${asset_url%%\?*}")"

installed_tag=""
[[ -f "$INSTALLED_MARKER" ]] && installed_tag="$(cat "$INSTALLED_MARKER")"
# Fall back to reading the current symlink target if the marker is absent.
if [[ -z "$installed_tag" && -L "$CURRENT_LINK" ]]; then
  installed_tag="$(basename "$(readlink "$CURRENT_LINK")")"
fi

if [[ "$latest_tag" == "$installed_tag" ]]; then
  log "already on latest release $latest_tag"
  exit 0
fi

[[ -n "$asset_url" ]] || fail "release $latest_tag has no $TARGET_TRIPLE .tar.gz asset"

log "updating: $installed_tag -> $latest_tag"

# --- download ---------------------------------------------------------------
tmp="$(mktemp -d "$STATE_DIR/.dl.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
tarball="$tmp/$asset_name"

# For a GitHub API asset URL, the octet-stream Accept header returns the binary.
dl_headers=(-H "Accept: application/octet-stream")
if command -v gh >/dev/null 2>&1 && [[ "$asset_url" == https://api.github.com/* ]]; then
  gh api "$asset_url" --header "Accept: application/octet-stream" > "$tarball" 2>>"$LOG_FILE" ||
    fail "asset download via gh failed"
else
  curl -fSL "${dl_headers[@]}" "$asset_url" -o "$tarball" 2>>"$LOG_FILE" ||
    fail "asset download failed"
fi

# --- verify checksum if the release ships one -------------------------------
sum_url="$(python3 -c '
import json,sys
d=json.load(open(sys.argv[1])); want=sys.argv[2]+".sha256"
for a in d.get("assets") or []:
    if a.get("name")==want:
        print(a.get("url") or a.get("browser_download_url") or ""); break' \
  "$STATE_DIR/.latest.json" "$asset_name")"
if [[ -n "$sum_url" ]]; then
  if command -v gh >/dev/null 2>&1 && [[ "$sum_url" == https://api.github.com/* ]]; then
    gh api "$sum_url" --header "Accept: application/octet-stream" > "$tmp/expected.sha256" 2>>"$LOG_FILE" || true
  else
    curl -fSL -H "Accept: application/octet-stream" "$sum_url" -o "$tmp/expected.sha256" 2>>"$LOG_FILE" || true
  fi
  if [[ -s "$tmp/expected.sha256" ]]; then
    expected="$(awk '{print $1}' "$tmp/expected.sha256")"
    actual="$(sha256sum "$tarball" | awk '{print $1}')"
    [[ "$expected" == "$actual" ]] || fail "checksum mismatch for $asset_name (expected $expected, got $actual)"
    log "checksum verified"
  fi
fi

# --- extract & install ------------------------------------------------------
tar -xzf "$tarball" -C "$tmp" || fail "extract failed"
# Tarball top dir is codex-<tag>-<triple>/bin/{codex,codex-code-mode-host}
pkg_dir="$(find "$tmp" -maxdepth 1 -type d -name "codex-*-${TARGET_TRIPLE}" | head -1)"
[[ -n "$pkg_dir" && -x "$pkg_dir/bin/codex" ]] || fail "unexpected package layout in $asset_name"

dest="$RELEASES_DIR/$latest_tag-$TARGET_TRIPLE"
rm -rf "$dest"
mkdir -p "$dest"
cp -a "$pkg_dir/bin" "$dest/bin"
chmod +x "$dest/bin/"* 2>/dev/null || true

# Atomic swap of the 'current' pointer, then the stable bin symlink.
ln -sfn "$dest" "$CURRENT_LINK.new"
mv -Tf "$CURRENT_LINK.new" "$CURRENT_LINK"
mkdir -p "$(dirname "$BIN_SYMLINK")"
ln -sfn "$CURRENT_LINK/bin/codex" "$BIN_SYMLINK"
echo "$latest_tag" > "$INSTALLED_MARKER"

# --- prune old fork releases (keep the newest N, never touch non-fork dirs) -
mapfile -t old < <(ls -1dt "$RELEASES_DIR"/fork-*-"$TARGET_TRIPLE" 2>/dev/null | tail -n +"$((KEEP_RELEASES+1))")
for d in "${old[@]:-}"; do
  [[ -n "$d" && "$(readlink -f "$CURRENT_LINK")" != "$(readlink -f "$d")" ]] && rm -rf "$d"
done

version="$("$CURRENT_LINK/bin/codex" --version 2>/dev/null | head -1 || echo "$latest_tag")"
notify normal "codex updated" "Installed fork release $latest_tag ($version)."
log "done: now on $latest_tag"
