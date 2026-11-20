#!/usr/bin/env bash
# Lists unseen completed attempts, the event inbox, or current activity with --active.
# The workflow triggers only on schedule and workflow_dispatch, so those two events cover it,
# whether or not main-validation.yaml is on the default branch yet.
# Output: run_id attempt, followed by API metadata outside --events mode.
set -euo pipefail
dir=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$dir" rev-parse --show-toplevel)
state=${OBSERVER_STATE_DIR:-$root/.local/ci-observer}
repo=golemcloud/golem
mode=${1:-completed}
[[ "$mode" == completed || "$mode" == --active || "$mode" == --events ]] || { echo 'usage: list-runs.sh [--active|--events]' >&2; exit 2; }
if [[ "$mode" == --events ]]; then
  shopt -s nullglob
  for record in "$state"/events/*.json; do
    jq -r 'select(.status == "pending" or .status == "notified") | [.run, .attempt] | @tsv' "$record"
  done | sort -n -k1,1 -k2,2 | while IFS=$'\t' read -r id attempt; do
    [[ -e "$state/seen/$id-a$attempt.json" ]] || printf '%s\t%s\n' "$id" "$attempt"
  done
  exit 0
fi
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
    record="$state/events/$id-a$a.json"
    if [[ -f "$record" ]] && jq -e '.status == "no-selection"' "$record" >/dev/null; then continue; fi
    [[ -e "$state/seen/$id-a$a.json" ]] || printf '%s\t%s\t%s\n' "$id" "$a" "$rest"
  done
done
