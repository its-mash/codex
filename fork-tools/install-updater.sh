#!/usr/bin/env bash
# install-updater.sh — install a systemd --user timer that keeps the local codex
# install on the fork's latest GitHub Release (see codex-update.sh).
#
# Works with OR without cloning the repo. No clone:
#   curl -fsSL https://raw.githubusercontent.com/its-mash/codex/main/fork-tools/install-updater.sh | bash
#
# Usage:
#   fork-tools/install-updater.sh            # install + enable hourly timer
#   fork-tools/install-updater.sh --uninstall
set -euo pipefail

HERE="$(cd "$(dirname "$(readlink -f "$0")")" 2>/dev/null && pwd || echo "")"
UNIT_DIR="$HOME/.config/systemd/user"
INTERVAL="${CODEX_UPDATE_INTERVAL:-hourly}" # onCalendar spec, e.g. hourly / *:0/30
RAW_BASE="${CODEX_FORK_RAW_BASE:-https://raw.githubusercontent.com/its-mash/codex/main/fork-tools}"

# The timer must point at a stable path, independent of any clone. Install the
# updater to the state dir: copy a sibling (cloned repo) or fetch it (no clone).
UPDATER="$HOME/.codex-fork-sync/codex-update.sh"

if [[ "${1:-}" == "--uninstall" ]]; then
  systemctl --user disable --now codex-update.timer 2>/dev/null || true
  rm -f "$UNIT_DIR/codex-update.service" "$UNIT_DIR/codex-update.timer"
  systemctl --user daemon-reload
  echo "codex-update timer removed (kept $UPDATER)."
  exit 0
fi

mkdir -p "$(dirname "$UPDATER")" "$UNIT_DIR"
if [[ -n "$HERE" && -f "$HERE/codex-update.sh" ]]; then
  cp -f "$HERE/codex-update.sh" "$UPDATER"
else
  curl -fsSL "$RAW_BASE/codex-update.sh" -o "$UPDATER"
fi
chmod +x "$UPDATER"

cat > "$UNIT_DIR/codex-update.service" <<EOF
[Unit]
Description=Update local codex to the fork's latest GitHub Release
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=$UPDATER
# notify-send needs the graphical session bus:
Environment=DISPLAY=:0
EOF

cat > "$UNIT_DIR/codex-update.timer" <<EOF
[Unit]
Description=Periodic codex fork auto-update

[Timer]
OnCalendar=$INTERVAL
OnBootSec=2min
Persistent=true

[Install]
WantedBy=timers.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now codex-update.timer
echo "Installed codex-update.timer (OnCalendar=$INTERVAL)."
echo "Run once now:  systemctl --user start codex-update.service"
echo "Watch logs:    journalctl --user -u codex-update.service -f"
