#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
source_commit="$(git rev-parse HEAD)"
runner_id="${GOLEM_BENCHMARK_RUNNER_ID:-github-actions-blacksmith-32vcpu}"
results_repository="${BENCHMARK_RESULTS_REPOSITORY:-https://github.com/golemcloud/benchmark-results.git}"
artifact_dir="${GOLEM_BENCHMARK_ARTIFACT_DIR:-$root/tmp}"
mkdir -p "$artifact_dir"
output="$(mktemp -d "$artifact_dir/ci-benchmark-analysis-XXXXXXXX")"

# This checkout is a read-only snapshot of published results, never a publisher.
git clone --depth 1 "$results_repository" "$output/benchmark-results"
git -C "$output/benchmark-results" rev-parse HEAD > "$output/results-commit.txt"
node "$root/integration-tests/benchmark_suites/analyze_ci_benchmarks.mjs" \
    "$output/benchmark-results" "$source_commit" "$runner_id" "$output"
echo "Analysis evidence: $output"
