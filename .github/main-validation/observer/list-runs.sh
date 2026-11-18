#!/usr/bin/env bash
# Lists unseen completed attempts, or current activity with --active.
# The workflow triggers only on schedule and workflow_dispatch, so those two events cover it,
# whether or not main-validation.yaml is on the default branch yet.
# Output: one TSV line per unseen attempt: run_id attempt event branch head_sha conclusion created_at url
set -euo pipefail
dir=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$dir" rev-parse --show-toplevel)
state=${OBSERVER_STATE_DIR:-$root/.local/ci-observer}
repo=golemcloud/golem
mode=${1:-completed}
[[ "$mode" == completed || "$mode" == --active ]] || { echo 'usage: list-runs.sh [--active]' >&2; exit 2; }
query='&status=completed'
[[ "$mode" != --active ]] || query=''
for event in schedule workflow_dispatch; do
  gh api "repos/$repo/actions/runs?per_page=100&event=$event$query" \
    | jq -r --arg mode "$mode" '.workflow_runs[] | select(.path | startswith(".github/workflows/main-validation.yaml")) | select($mode != "--active" or .status != "completed") | [.id, .run_attempt, .event, .head_branch, .head_sha, .conclusion, .created_at, .html_url] | @tsv' || exit 1
done | sort -n | while IFS=$'\t' read -r id attempt rest; do
  if [[ "$mode" == --active ]]; then
    printf '%s\t%s\t%s\n' "$id" "$attempt" "$rest"
    continue
  fi
  # Earlier attempts of a re-run run are reported too, if they were never seen.
  for ((a = 1; a <= attempt; a++)); do
    [[ -e "$state/seen/$id-a$a.json" ]] || printf '%s\t%s\t%s\n' "$id" "$a" "$rest"
  done
done
