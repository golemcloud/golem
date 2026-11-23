# Offline R3 metric validation

`validate_r3_metrics.py` reads saved detailed schema-2 CI records. It has no network,
measurement, subprocess, build, publication or rollout entry point. It uses Python 3.9+
and the standard library. It does not change the paired comparison or published-result
consumer. All new runs and rollout remain separately gated.

## Boundaries and inputs

The interpretation is `r3-metric-contract-v1`. Its manifest declares all 84 configurations
of the reviewed CI suite, including fixed metrics and possible routing cells that never
appeared in saved results. The suite declaration reproduces the producer's serialized
fingerprint. Digests bind the declaration to the suite file and the producer source files.
The manifest also hashes the analyzer itself. A changed producer or suite requires review;
there is no fallback to an observed-key inventory or another source contract.

This implementation interprets the retained CI suite at source
`eb46c797c329e1787fb9260cc57b132e0830489d`, harness version 1, with three requested
iterations. It does not authorize that N or accept a future suite implicitly. It rejects
unknown sources, missing configurations, missing identities and failed lifecycle evidence.
Artifact digests and hardware/series identities separate comparable groups. Recorded
artifact declarations are checked for completeness and equality; artifact bytes are not
independently rehashed by this command.

Supply each original record and its SHA-256 from retained evidence receipts, not a hash
chosen after editing the record. Each input must contain exactly one run. Duplicate run
IDs, duplicate JSON keys and input hash disagreements fail validation. A merged suite's
run ID identifies its saved execution; it does not independently prove fresh processes or
host independence for its partitions.

```shell copy Copy Offline Validator Command
python3 integration-tests/benchmark_suites/validate_r3_metrics.py \
  --record <saved-result.json> <retained-sha256> \
  --record <another-saved-result.json> <retained-sha256> \
  --output <new-analysis-directory>
```

The output directory must not exist. `manifest.json` and `report.json` are a separate
interpretation; neither raw evidence nor old analysis is overwritten. Exit 0 means the
records passed metric validity checks, not coverage, noise, parallel-control or R3
acceptance. Invalid identities or iterations produce exit 2 and a report. Unreadable,
malformed or hash-mismatched files and changed producer declarations fail before output.

## Interpretation

- Fixed/core metrics require their source-defined sample cardinality in every requested
  iteration, including iteration zero. Missing metrics cannot disappear from the manifest.
  Failures, skips, recovered calls, retries, timeouts and malformed numbers are invalid.
- Nonaggregate RPC must account for every target and call across local/remote primary
  samples and per-target secondary samples. Overall throughput remains mandatory. Only
  complete accounting can establish zero route exposure. That observation is N/A, never
  zero latency. One-executor remote routing is structurally N/A; a possible multi-executor
  route still requires five exposed process estimates.
- Durations use a median within each applicable iteration, then a median of those
  iteration estimates per process. Event counts sum within each iteration. Rates are
  exactly one scalar despite their count-vector wire encoding. No request pooling or
  route pooling supplies extra replication. Requested indexes, applicability masks,
  exposures and reasons remain visible.
- Detailed recovery storage counters form a sparse family. Include every exact observed
  label, not an intersection. A missing label becomes an annotated producer-zero only
  after a successful expected phase with both explicit page/session recovery witnesses.
  Never extend that rule to witnesses, coarse counts, durations or whole missing phases.
  These are executor-wide logical API calls, including failures before backend retries,
  under generation-reset semantics—not physical I/O or foreground-only work. Original
  scrape bodies are not retained, so this inference is producer-backed.
- Comparable groups need five exposed process estimates for descriptive sample-CV
  evaluation against 10%. CV uses max-normalized process estimates so intermediate
  rounding cannot turn subnormal variability into a zero standard deviation.
  All-zero event counts may be stable; zero-rounded rates and
  durations are under-resolution. Mixed zero/positive counts receive no relaxed rule.
  Max/min and the 1.20 control threshold are reported, but the command does not certify
  that observations are comparable parallel controls. That requires campaign receipts.
  An unrepresentable max/min is retained as null with an explicit status and cannot pass
  that threshold. It is never capped to a favourable value.

`r3_acceptance` and `n_accepted` remain false in every report. The tool validates the metric
contract and reports blockers; it does not certify campaign execution, independence,
control overlap, budget compliance or optimization acceptance. Historical failures remain
failures. A new measurement campaign, changed N, workflow adaptation and rollout require
separate approval.

## CI routing-coverage pilot

`r3_coverage.py` is a separate, CI-only experiment runner. It reuses this validator's
source-defined rules, with an explicit research suite/fingerprint, five iterations and
the exact two-fixture inventory. The original offline CLI remains pinned to the full N=3
suite. No missing metric or route is removed from the production inventory.

The pilot tests four configurations containing all six historically underexposed routing
cells: CPU-intensive, cluster 5, sizes 1/10, length 100; large-input, cluster 5, size 1,
lengths 100/10000. Four fresh hosts each run one suite process. It reports actual
local/remote exposure and complete call accounting, never R3 or N acceptance. Missing
exposure is a valid coverage failure, not a reason to invent zero latency or repeat a
process. A failed/missing host or any unexposed possible route stops progression to a
full-suite campaign. A favourable pilot cannot supply full-suite replications or establish
future routing guarantees. Full waves would still stop on their first coverage failure.

Dispatch the existing benchmark workflow with `coverage_pilot=true`, `publish=false`, all
other experiment flags false, and a fixed epoch deadline no later than run creation plus
30 minutes. The reusable workflow runs only four 32-vCPU jobs, each capped at 25 minutes;
the harness process is capped at 15 minutes with a two-minute deadline reserve. It starts
the reviewed ordinary-cancel/status/force-cancel guard before checkout. Cancellation and
artifact preservation remain dependent on GitHub/runner availability; forced cancellation
can prevent evidence upload. This is not a provider-enforced invoice or physical shutdown
guarantee.

The experiment uses original run 44078542455/artifact 14329150891, verifies the canonical
producer workflow and archive/member hashes, and uses the original runtime checkout,
Redis 8.0.2, pinned PostgreSQL digest and hardware fingerprint. It never rebuilds or applies
an overlay. Origin, installed-member, suite, raw-result, timing and cleanup receipts are
retained. Leftover containers fail closed; only the owned process group is signalled.
Fresh ephemeral runner teardown owns any remaining container cleanup.

The pilot allocates $10 of the proposed $150 total before tax: four jobs ×26 billed
minutes ×$0.064 = $6.656 modeled compute, at most 1 GiB of evidence with seven-day
retention, and the remaining allocation as contingency. Evidence over 250 MiB per host
fails upload eligibility and retains rejection receipts rather than truncating results.
No retries, replacement hosts, build charges or paid runner add-ons are authorized by
this path. Public tariff and resource caps bound the model, not the actual provider
invoice. Reconcile observed usage before allocating any full-suite waves.
