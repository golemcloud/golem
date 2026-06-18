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

Each iteration file is a BenchmarkSuiteResultCollection JSON written by the
benchmark binary's `density --action cell` (one run, one result). Pooling:

  * count results  -> avg/min/max recomputed over the concatenated `all` arrays
                      across iterations (each iteration's `all` is its samples).
  * duration results (invoke-latency) -> percentiles recomputed over the
                      concatenated `all` arrays (durations are JSON numbers in
                      milliseconds with fraction).

The first iteration's collection is used as the template (suite/run metadata,
result name/description); only the per-key count/duration stats are replaced
with the pooled values. The pooled `all` arrays are retained so a downstream
consumer can re-pool if needed.

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


def result_of(collection):
    # collection -> runs[0] -> results[0]
    return collection["runs"][0]["results"][0]


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

    # Template from the first iteration.
    merged = collections[0]
    merged_result = result_of(merged)

    # ── Pool count results ────────────────────────────────────────────────────
    pooled_counts = {}
    for coll in collections:
        for key, cr in result_of(coll).get("countResults", {}).items():
            pooled_counts.setdefault(key, []).extend(cr.get("all", []))

    for key, all_vals in pooled_counts.items():
        if not all_vals:
            continue
        merged_result.setdefault("countResults", {})[key] = {
            "avg": sum(all_vals) // len(all_vals),
            "min": min(all_vals),
            "max": max(all_vals),
            "all": all_vals,
        }

    # ── Pool duration results (milliseconds) ───────────────────────────────────
    pooled_durations = {}
    for coll in collections:
        for key, dr in result_of(coll).get("durationResults", {}).items():
            pooled_durations.setdefault(key, []).extend(dr.get("all", []))

    for key, all_vals in pooled_durations.items():
        if not all_vals:
            continue
        s = sorted(all_vals)
        merged_result.setdefault("durationResults", {})[key] = {
            "avg": sum(s) / len(s),
            "min": s[0],
            "max": s[-1],
            "median": percentile(s, 50.0),
            "p90": percentile(s, 90.0),
            "p95": percentile(s, 95.0),
            "p99": percentile(s, 99.0),
            "all": s,
        }

    with open(out_path, "w") as fh:
        json.dump(merged, fh, indent=2)

    sys.stderr.write(
        f"Pooled {len(iter_paths)} iterations into {out_path} "
        f"({len(pooled_counts)} count keys, {len(pooled_durations)} duration keys)\n"
    )


if __name__ == "__main__":
    main()
