#!/usr/bin/env bash
# Collects the facts of one Main validation run attempt into runs/<id>-a<attempt>/facts.json:
# attempt conclusion, jobs and failed steps, the exact tested source and status context (from the
# select/result job logs), and a CTRF summary of failed and recovered tests.
# Artifacts are downloaded into a fresh directory and treated as untrusted data.
# Usage: run-facts.sh <run_id> <attempt>
set -euo pipefail
dir=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$dir" rev-parse --show-toplevel)
state=${OBSERVER_STATE_DIR:-$root/.local/ci-observer}
repo=golemcloud/golem
id=$1
attempt=$2
out="$state/runs/$id-a$attempt"
rm -rf "$out"
mkdir -p "$out/artifacts" "$out/logs"

gh api "repos/$repo/actions/runs/$id/attempts/$attempt" > "$out/attempt.json"
gh api --paginate --slurp "repos/$repo/actions/runs/$id/attempts/$attempt/jobs?per_page=100" \
  | jq '[.[].jobs[]]' > "$out/jobs.json"
gh api --paginate --slurp "repos/$repo/actions/runs/$id/artifacts?per_page=100" \
  | jq '[.[].artifacts[]]' > "$out/artifacts.json"

job_id() { jq -r --arg n "$1" '[.[] | select(.name == $n)][0].id // ""' "$out/jobs.json"; }
for job in select result linux; do
  jid=$(job_id "$job")
  if [[ -n "$jid" ]]; then
    if ! gh api --allow-escape-sequences "repos/$repo/actions/jobs/$jid/logs" > "$out/logs/$job.log" 2>/dev/null; then
      echo "Job log unavailable: $job ($jid); source and analysis may be incomplete" >&2
    fi
  fi
done

# The result job's env block prints SOURCE and CONTEXT; the linux checkout prints the ref.
source=$(grep -hoE '^[^ ]+ +SOURCE: [0-9a-f]{40}' "$out/logs/result.log" 2>/dev/null | head -1 | grep -oE '[0-9a-f]{40}' || true)
[[ -n "$source" ]] || source=$(grep -hoE '^[^ ]+ +ref: [0-9a-f]{40}' "$out/logs/linux.log" 2>/dev/null | head -1 | grep -oE '[0-9a-f]{40}' || true)
context=$(grep -hoE '^[^ ]+ +CONTEXT: .*' "$out/logs/result.log" 2>/dev/null | head -1 | sed -E 's/^[^ ]+ +CONTEXT: //' || true)
request=$(grep -hoE '^[^ ]+ +REQUEST: .*' "$out/logs/result.log" 2>/dev/null | head -1 | sed -E 's/^[^ ]+ +REQUEST: //' || true)

for name in "main-validation-linux-$id" "main-validation-apfs-$id"; do
  if jq -e --arg n "$name" 'any(.[]; .name == $n and (.expired | not))' "$out/artifacts.json" >/dev/null; then
    mkdir -p "$out/artifacts/$name"
    gh run download "$id" -R "$repo" -n "$name" -D "$out/artifacts/$name" >/dev/null 2>&1 || echo "download failed: $name" >&2
  fi
done

python3 -I "$dir/ctrf-summary.py" "$out/artifacts" > "$out/ctrf.json"

jq -n \
  --slurpfile attempt "$out/attempt.json" \
  --slurpfile jobs "$out/jobs.json" \
  --slurpfile ctrf "$out/ctrf.json" \
  --arg source "$source" --arg context "$context" --arg request "$request" '
  ($jobs[0] | map({key: .name, value: .}) | from_entries) as $j |
  {
    run_id: $attempt[0].id, attempt: $attempt[0].run_attempt, url: $attempt[0].html_url,
    event: $attempt[0].event, workflow_sha: $attempt[0].head_sha, branch: $attempt[0].head_branch,
    status: $attempt[0].status, conclusion: $attempt[0].conclusion,
    created_at: $attempt[0].created_at, updated_at: $attempt[0].updated_at,
    selected: (if $attempt[0].status != "completed" then null else (($j.result.conclusion // "skipped") != "skipped") end),
    source: $source, context: $context, request: $request,
    jobs: [$jobs[0][] | {name, conclusion, started_at, completed_at,
      failed_steps: [.steps[]? | select(.conclusion == "failure") | .name]}],
    tests: $ctrf[0]
  }' > "$out/facts.json"
cat "$out/facts.json"
