#!/usr/bin/env bash
# Disable every inherited openai/codex workflow on the fork, keeping only
# `fork-sync-release`. Disabling is admin-only, so run this with an its-mash
# token that has admin on the repo (the gh login here, mr-benty, is pull-only).
#
# Usage (token stays in your shell, never in this repo/chat):
#   GH_TOKEN=<its-mash PAT with repo+workflow> fork-tools/disable-inherited-workflows.sh
#
# It is idempotent — re-running only disables anything still active.
set -euo pipefail

REPO="${CODEX_FORK_REPO_SLUG:-its-mash/codex}"
KEEP="${CODEX_FORK_KEEP_WORKFLOW:-fork-sync-release}"

command -v gh >/dev/null || { echo "gh CLI required" >&2; exit 1; }

# Confirm the token can actually administer the repo before touching anything.
admin="$(gh api "repos/$REPO" --jq '.permissions.admin' 2>/dev/null || echo false)"
if [ "$admin" != "true" ]; then
  echo "The active token cannot administer $REPO (need admin:true). Provide an its-mash token:" >&2
  echo "  GH_TOKEN=<its-mash PAT> $0" >&2
  exit 1
fi

echo "Disabling inherited workflows on $REPO (keeping '$KEEP')..."
gh api --paginate "repos/$REPO/actions/workflows" \
  --jq '.workflows[] | select(.state=="active") | [.id, .name] | @tsv' |
while IFS=$'\t' read -r id name; do
  if [ "$name" = "$KEEP" ]; then
    echo "  keep    : $name"
    continue
  fi
  if gh api -X PUT "repos/$REPO/actions/workflows/$id/disable" 2>/dev/null; then
    echo "  disabled: $name"
  else
    echo "  FAILED  : $name" >&2
  fi
done
echo "Done. '$KEEP' remains enabled."
