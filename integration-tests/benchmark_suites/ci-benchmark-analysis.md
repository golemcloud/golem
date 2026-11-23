# CI benchmark analysis

GitHub Actions owns nightly performance measurement and publication through
`.github/workflows/benchmark.yaml`. Orbs consume its saved results for analysis and
investigation. They never build or execute performance workloads, publish measurements,
or fall back to local measurements when CI evidence is missing.

Run `cargo make analyze-ci-benchmarks` from the Golem commit to investigate. The command
clones a read-only snapshot of `golemcloud/benchmark-results`, selects the newest published
`CI` result for the exact Golem commit and `github-actions-blacksmith-32vcpu` runner, and
uses that repository's analyzer. It verifies the selected detail record and retains the
results repository commit, detail hash, failures and analysis under a unique `tmp/` directory.
It does not substitute another Golem commit. A missing result produces `missing-ci-result`,
with `shouldAlert: false`, without starting a build, benchmark, dispatch or publication.

`GOLEM_BENCHMARK_ARTIFACT_DIR` changes the evidence directory.
`BENCHMARK_RESULTS_REPOSITORY` selects a results repository for local fixture validation.
`GOLEM_BENCHMARK_RUNNER_ID` selects the CI runner identity; it is not a measurement option.
No results write token is required by this consumer.

## Analysis trigger

Keep the former orb measurement schedule disabled until the schema-2 reader, writer and
publication path are live. An analysis-only trigger may then run the consumer after CI
publishes. Follow the main validation CI producer/analysis pattern. A trigger may report
missing evidence, but must not dispatch or run benchmarks, clean caches or publish results.

The analysis task should fetch Golem history, select the exact commit of the published CI
run to investigate, and run `cargo make analyze-ci-benchmarks`. Report the selected commit,
results snapshot and evidence paths. Never substitute the orb's historical runner series.

## Investigation and warnings

Preserve failure information from `selection.json` alongside `analysis.json`; a successful
analysis is not proof that the measured suite succeeded. Treat the analyzer's `shouldAlert`
boolean as the notification decision, not the status name or presence of candidates.
GOL-1192 owns the new confidence interval, regression-state, waiver and reporting policy;
this consumer does not introduce a competing policy.

When investigation is needed, inspect every candidate and its baseline source commits.
Read changes on the affected execution paths. Distinguish direct evidence from inference,
consider runner/workload changes, and say when no commit is a credible cause.

Retain the existing Slack warning deduplication in the analysis thread. Key warnings by
runner, suite and run timestamp, so reanalysis does not alert twice. Use the existing
restricted `SLACK_BENCHMARK_WEBHOOK_URL` secret without exposing it. Include affected
benchmarks, percentage changes, baseline/latest commit links, suspected cause, confidence,
and <https://golemcloud.github.io/benchmark-results/>. Do not send a warning merely because
a run is missing or the consumer fails.

Historical orb measurements remain historical evidence. R3, paired base/head comparisons
and further performance diagnostics execute on CI runners. A future dedicated executor
can replace the runner behind GitHub Actions as separate infrastructure work.
