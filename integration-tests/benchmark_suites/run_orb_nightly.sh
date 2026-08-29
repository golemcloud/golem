#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

runner_id="${GOLEM_BENCHMARK_RUNNER_ID:-amp-orb-a1.xxlarge}"
expected_ref="${GOLEM_BENCHMARK_EXPECTED_REF:-refs/heads/main}"
source_commit="$(git rev-parse HEAD)"
source_ref="$(git symbolic-ref -q HEAD || true)"
artifact_dir="${GOLEM_BENCHMARK_ARTIFACT_DIR:-$root/tmp}"
results_repository="${BENCHMARK_RESULTS_REPOSITORY:-https://github.com/golemcloud/benchmark-results.git}"
generated_files=(
    test-components/benchmarks/package-lock.json
    test-components/benchmarks/Cargo.lock
)

if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
    echo "Refusing to run with tracked worktree changes" >&2
    git status --short >&2
    exit 1
fi
if [[ "$source_ref" != "$expected_ref" ]]; then
    echo "Refusing to publish $source_ref; expected $expected_ref" >&2
    exit 1
fi

mkdir -p "$artifact_dir"
artifact_dir="$(realpath "$artifact_dir")"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
results="$artifact_dir/orb-benchmark-results-${timestamp}-${source_commit:0:12}.json"
log="$artifact_dir/orb-benchmark-${timestamp}-${source_commit:0:12}.log"
analysis="$artifact_dir/orb-benchmark-analysis-${timestamp}-${source_commit:0:12}.json"

restore_generated_files() {
    git restore -- "${generated_files[@]}"
}

cleanup() {
    status=$?
    trap - EXIT
    restore_generated_files || status=1
    exit "$status"
}
trap cleanup EXIT

if [[ -n "${GOLEM_BENCHMARK_RESULTS_INPUT:-}" ]]; then
    results="$(realpath "$GOLEM_BENCHMARK_RESULTS_INPUT")"
    analysis="$artifact_dir/orb-benchmark-analysis-retry-${source_commit:0:12}.json"
    echo "Using existing benchmark artifact $results"
else
    amp orb services ensure
    export PATH="/opt/node24/bin:$HOME/.cargo/bin:$PATH"
    export WASI_SDK_VERSION=25
    export WASI_SDK_PATH=/opt/wasi-sdk
    WASM_RQUICKJS_VERSION="$(awk -F'"' '/^[[:space:]]*WASM_RQUICKJS_VERSION: / { print $2; exit }' .github/workflows/benchmark.yaml)"
    if [[ -z "$WASM_RQUICKJS_VERSION" ]]; then
        echo "Failed to read WASM_RQUICKJS_VERSION from .github/workflows/benchmark.yaml" >&2
        exit 1
    fi
    export WASM_RQUICKJS_VERSION
    if [[ "$(wasm-rquickjs --version 2>/dev/null || true)" != "wasm-rquickjs-cli $WASM_RQUICKJS_VERSION" ]]; then
        cargo binstall --force --locked "wasm-rquickjs-cli@$WASM_RQUICKJS_VERSION"
    fi
    export GOLEM_BENCHMARK_RESULTS_PATH="$results"

    cargo clean

    mapfile -t postgres_containers < <(
        docker ps --all --quiet --filter ancestor=postgres:17.7
    )
    if ((${#postgres_containers[@]})); then
        docker rm --force --volumes "${postgres_containers[@]}"
    fi
    docker container prune --force
    docker volume prune --force

    # The SDK build currently caches generated output without tracking its source revision.
    cargo make clean-sdk-ts
    set +e
    cargo make run-benchmark-suite-orb 2>&1 | tee "$log"
    benchmark_status=${PIPESTATUS[0]}
    set -e
    if ((benchmark_status != 0)); then
        echo "Benchmark failed with status $benchmark_status; partial results will not be published" >&2
        exit "$benchmark_status"
    fi
fi

restore_generated_files
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
    echo "Benchmark unexpectedly changed tracked files" >&2
    git status --short >&2
    exit 1
fi
if ! run_timestamp="$(jq -er \
        --arg runner "$runner_id" \
        --arg commit "$source_commit" \
        --arg ref "$source_ref" \
        '.runs | select(length == 1) | .[0]
            | select(.suite == "CI")
            | select(.runner.id == $runner)
            | select(.source.repository == "golemcloud/golem")
            | select(.source.commitSha == $commit and .source.ref == $ref)
            | .timestamp' \
        "$results")"; then
    echo "Benchmark artifact does not match the current runner, commit, and ref" >&2
    exit 1
fi

if [[ -n "${BENCHMARK_RESULTS_TOKEN:-}" ]]; then
    authorization="$(printf 'x-access-token:%s' "$BENCHMARK_RESULTS_TOKEN" | base64 -w0)"
    export GIT_CONFIG_COUNT=1
    export GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
    export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $authorization"
fi

publish_root="$artifact_dir/benchmark-results-publish"
for attempt in 1 2 3; do
    rm -rf "$publish_root"
    if git clone --depth 1 "$results_repository" "$publish_root" && (
        cd "$publish_root"
        node scripts/append-results.mjs "$results"
        if git diff --quiet -- results/results.json; then
            echo "Benchmark run is already published at $(git rev-parse HEAD)"
            exit 0
        fi

        npm ci
        npm test
        npm run build
        git add results/results.json
        if [[ "$(git diff --cached --name-only)" != "results/results.json" ]]; then
            echo "Publisher attempted to commit unexpected files" >&2
            exit 1
        fi
        git -c user.name="Golem Benchmark Bot" \
            -c user.email="benchmark-bot@golem.cloud" \
            commit -m "Append Amp orb benchmark results for ${source_commit:0:12}"
        git push origin HEAD:master
    ); then
        published_commit="$(git -C "$publish_root" rev-parse HEAD)"
        git -C "$publish_root" fetch origin master
        if ! git -C "$publish_root" merge-base --is-ancestor \
            "$published_commit" origin/master; then
            echo "Published commit $published_commit is not on remote master" >&2
            exit 1
        fi
        node "$publish_root/scripts/analyze-regressions.mjs" \
            "$publish_root/results/results.json" \
            --runner "$runner_id" \
            --suite CI \
            --timestamp "$run_timestamp" \
            --output "$analysis"
        jq -e '.status != "run-not-found" and .status != "no-runs"' "$analysis" >/dev/null
        echo "Published benchmark results at $published_commit"
        echo "Results: $results"
        echo "Analysis: $analysis"
        [[ -f "$log" ]] && echo "Log: $log"
        exit 0
    fi
    echo "Publish attempt $attempt lost a race or failed; retrying from current master" >&2
done

echo "Failed to publish benchmark results after three attempts" >&2
exit 1
