#!/usr/bin/env python3
# Copyright 2024-2026 Golem Cloud
#
# Licensed under the Golem Source License v1.1 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://license.golem.cloud/LICENSE
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Pools N density per-iteration result files into one combined cell result.

Each iteration file is a `BenchmarkSuiteResultCollection` JSON written by the
benchmark binary's `density --action cell`. Its shape (snake_case at the
`BenchmarkRunResult` level) is:

  runs[0].results[0].results[*].count_results[<key>]    -> {avg,min,max,all}
  runs[0].results[0].results[*].duration_results[<key>] -> {avg,...,p99,all}

i.e. `runs[0].results[0]` is a `BenchmarkResult`, whose own `results[*]` are the
per-run `BenchmarkRunResult`s that actually carry the count/duration maps.

Pooling concatenates the `all` arrays for each key across every run-result of
every iteration, then recomputes:

  * count results  -> avg/min/max over the pooled samples.
  * duration results (invoke-latency) -> percentiles over the pooled samples
                      (durations are JSON numbers in milliseconds with fraction).

The first iteration's collection is the template (suite/run metadata, result
name/description); the pooled stats are written into its first run-result and
the remaining run-results are dropped so the cell has a single combined view.

Usage:
  merge_density_iterations.py <output.json> <iter1.json> [iter2.json ...]
"""

import json
import sys


def percentile(sorted_values, k):
    if not sorted_values:
        return 0.0
    n = len(sorted_values)
    if n == 1:
        return sorted_values[0]
    p = (k / 100.0) * (n - 1)
    lo = int(p)
    hi = min(lo + 1, n - 1)
    frac = p - lo
    return sorted_values[lo] + (sorted_values[hi] - sorted_values[lo]) * frac


def benchmark_result_of(collection):
    # collection -> runs[0] -> results[0] : a BenchmarkResult
    return collection["runs"][0]["results"][0]


def run_results_of(collection):
    # The BenchmarkRunResults that carry the count/duration maps.
    return benchmark_result_of(collection).get("results", [])


def main():
    if len(sys.argv) < 3:
        sys.stderr.write(
            "usage: merge_density_iterations.py <output.json> <iter1.json> [iter2.json ...]\n"
        )
        sys.exit(2)

    out_path = sys.argv[1]
    iter_paths = sys.argv[2:]

    collections = []
    for p in iter_paths:
        with open(p) as fh:
            collections.append(json.load(fh))

    # Template from the first iteration; pooled stats are written into its
    # first run-result, which becomes the cell's single combined run-result.
    merged = collections[0]
    merged_run_results = run_results_of(merged)
    if not merged_run_results:
        sys.stderr.write(
            f"WARNING: {iter_paths[0]} has no run results; writing template unchanged\n"
        )
        with open(out_path, "w") as fh:
            json.dump(merged, fh, indent=2)
        return
    merged_run_result = merged_run_results[0]

    # ── Pool count results ────────────────────────────────────────────────────
    pooled_counts = {}
    for coll in collections:
        for run_result in run_results_of(coll):
            for key, cr in run_result.get("count_results", {}).items():
                pooled_counts.setdefault(key, []).extend(cr.get("all", []))

    merged_counts = {}
    for key, all_vals in pooled_counts.items():
        if not all_vals:
            continue
        merged_counts[key] = {
            "avg": sum(all_vals) // len(all_vals),
            "min": min(all_vals),
            "max": max(all_vals),
            "all": all_vals,
        }
    merged_run_result["count_results"] = merged_counts

    # ── Pool duration results (milliseconds) ───────────────────────────────────
    pooled_durations = {}
    for coll in collections:
        for run_result in run_results_of(coll):
            for key, dr in run_result.get("duration_results", {}).items():
                pooled_durations.setdefault(key, []).extend(dr.get("all", []))

    merged_durations = {}
    for key, all_vals in pooled_durations.items():
        if not all_vals:
            continue
        s = sorted(all_vals)
        merged_durations[key] = {
            "avg": sum(s) / len(s),
            "min": s[0],
            "max": s[-1],
            "median": percentile(s, 50.0),
            "p90": percentile(s, 90.0),
            "p95": percentile(s, 95.0),
            "p99": percentile(s, 99.0),
            "all": s,
        }
    merged_run_result["duration_results"] = merged_durations

    # Collapse to the single combined run-result.
    benchmark_result_of(merged)["results"] = [merged_run_result]

    with open(out_path, "w") as fh:
        json.dump(merged, fh, indent=2)

    sys.stderr.write(
        f"Pooled {len(iter_paths)} iterations into {out_path} "
        f"({len(pooled_counts)} count keys, {len(pooled_durations)} duration keys)\n"
    )


if __name__ == "__main__":
    main()
