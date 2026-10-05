#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

results="$(realpath "${GOLEM_BENCHMARK_RESULTS_INPUT:?GOLEM_BENCHMARK_RESULTS_INPUT is required}")"
runner_id="${GOLEM_BENCHMARK_RUNNER_ID:?GOLEM_BENCHMARK_RUNNER_ID is required}"
suite="${GOLEM_BENCHMARK_SUITE:-CI}"
results_repository="${BENCHMARK_RESULTS_REPOSITORY:-https://github.com/golemcloud/benchmark-results.git}"
analysis="${GOLEM_BENCHMARK_RESULTS_ANALYSIS:-$root/tmp/benchmark-analysis.json}"
publish_root="${GOLEM_BENCHMARK_PUBLISH_DIR:-$root/tmp/benchmark-results-publish}"
mkdir -p "$(dirname "$analysis")" "$(dirname "$publish_root")"
analysis="$(realpath -m "$analysis")"
publish_root="$(realpath -m "$publish_root")"

if ! run_timestamp="$(jq -er \
        --arg runner "$runner_id" \
        --arg suite "$suite" \
        '.runs | select(length == 1) | .[0]
            | select(.runner.id == $runner and .suite == $suite)
            | .timestamp' \
        "$results")"; then
    echo "Benchmark artifact does not contain one $suite run for runner $runner_id" >&2
    exit 1
fi
source_commit="$(jq -er '.runs[0].source.commitSha' "$results")"

if [[ -n "${BENCHMARK_RESULTS_TOKEN:-}" ]]; then
    authorization="$(printf 'x-access-token:%s' "$BENCHMARK_RESULTS_TOKEN" | base64 -w0)"
    export GIT_CONFIG_COUNT=1
    export GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
    export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $authorization"
fi

published=false
for attempt in 1 2 3; do
    cd "$root"
    rm -rf "$publish_root"
    if ! git clone --depth 1 "$results_repository" "$publish_root"; then
        echo "Publish attempt $attempt could not clone current master; retrying" >&2
        continue
    fi

    cd "$publish_root"
    node scripts/append-results.mjs "$results"
    if git diff --quiet -- public/data/index.json; then
        echo "Benchmark run is already published at $(git rev-parse HEAD)"
        published=true
        break
    fi

    npm ci
    npm test
    npm run build
    git add public/data

    mapfile -t staged_files < <(git diff --cached --name-only)
    if ((${#staged_files[@]} != 2)) ||
        [[ ! " ${staged_files[*]} " =~ " public/data/index.json " ]]; then
        echo "Publisher attempted to commit unexpected files" >&2
        exit 1
    fi
    for staged_file in "${staged_files[@]}"; do
        if [[ "$staged_file" != "public/data/index.json" &&
            ! "$staged_file" =~ ^public/data/runs/[0-9a-f]{24}\.json$ ]]; then
            echo "Publisher attempted to commit unexpected file $staged_file" >&2
            exit 1
        fi
    done

    git -c user.name="Golem Benchmark Bot" \
        -c user.email="benchmark-bot@golem.cloud" \
        commit -m "Append $runner_id benchmark results for ${source_commit:0:12}"
    if git push origin HEAD:master; then
        published=true
        break
    fi
    echo "Publish attempt $attempt lost a race; retrying from current master" >&2
done

if [[ "$published" != true ]]; then
    echo "Failed to publish benchmark results after three attempts" >&2
    exit 1
fi

published_commit="$(git rev-parse HEAD)"
git fetch origin master
if ! git merge-base --is-ancestor "$published_commit" origin/master; then
    echo "Published commit $published_commit is not on remote master" >&2
    exit 1
fi

node scripts/analyze-regressions.mjs \
    public/data/index.json \
    --runner "$runner_id" \
    --suite "$suite" \
    --timestamp "$run_timestamp" \
    --output "$analysis"
jq -e '.status != "run-not-found" and .status != "no-runs"' "$analysis" >/dev/null

echo "Published benchmark results at $published_commit"
echo "Results: $results"
echo "Analysis: $analysis"
