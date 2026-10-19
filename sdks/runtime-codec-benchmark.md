# Runtime codec preparation measurements

Baseline: [the exact fix commit](https://github.com/golemcloud/golem/commit/a91a0bcff33fd621132e686acd8cb77cdd687ce4),
not `main`. Measurements were collected on 2026-10-06 in the same Linux x86-64
orb with Node 24.19.0, Intel Xeon 2.60 GHz, Effect 4.0.0 / Vitest 5.0.1 and
non-Effect SDK Vitest 3.2.4. Both checkouts used the same lockfiles and installed
dependencies. No builds or other test suites ran during measurement.

## Method

The checked-in `codec-invocation.bench.test.ts` files are opt-in microbenchmarks.
Copy the identical benchmark file into an exact-baseline worktree before running
the same command there. In each SDK directory:

```sh
# sdks/effect
CODEC_BENCH=1 npx vitest run test/codec-invocation.bench.test.ts --reporter=verbose --silent=false

# sdks/ts/packages/golem-ts-sdk
CODEC_BENCH=1 npx pnpm exec vitest run tests/codec-invocation.bench.test.ts --reporter=verbose --silent=false
```

Registration and input fixture preparation are excluded. Payloads contain either
1 or 64 records with an asymmetric string and number. Each operation has 1,000
warmup iterations, then seven batches of 2,000 operations. Each batch reports
microseconds per operation, including the common benchmark `await`. Two separate
process runs per checkout were measured sequentially. Values below are medians
of the fourteen batch averages, not latency percentiles. There is no timing
assertion or CI performance gate.

## Results (µs/op; lower is better)

| SDK | Records | Operation | Baseline | Optimized |
| --- | ---: | --- | ---: | ---: |
| Effect | 1 | Decode | 9.96 | 7.50 |
| Effect | 1 | Encode | 8.76 | 7.88 |
| Effect | 1 | Echo invocation | 13.30 | 13.29 |
| Effect | 64 | Decode | 48.89 | 43.70 |
| Effect | 64 | Encode | 83.43 | 82.57 |
| Effect | 64 | Echo invocation | 134.23 | 128.61 |
| Non-Effect | 1 | Decode | 2.27 | 2.53 |
| Non-Effect | 1 | Encode | 2.62 | 1.54 |
| Non-Effect | 1 | Echo invocation | 3.09 | 2.69 |
| Non-Effect | 64 | Decode | 23.91 | 25.12 |
| Non-Effect | 64 | Encode | 44.61 | 22.16 |
| Non-Effect | 64 | Echo invocation | 72.50 | 45.12 |

The non-Effect 64-record encoding improvement is consistent across both runs:
baseline run medians 45.22 / 43.25, optimized 22.11 / 22.37. Echo invocation
medians were 67.51 / 75.97 versus 45.08 / 47.30. Skipping the async resource
preparation walk for codecs already known to be resource-free explains this
reduction. Ordinary `toValue` validation and generic wire flattening remain.

Effect decoder run medians were 8.64 / 10.69 versus 7.22 / 7.78 for one record,
and 47.87 / 49.46 versus 42.71 / 47.55 for 64 records. Effect echo invocation
does **not** show a reliable improvement: 64-record medians were 134.10 / 135.18
versus 121.71 / 137.94, and single-record invocation was effectively unchanged.
Non-Effect decoder results also overlap; no decoder speedup is claimed there.

## Correctness and limits

- Effect: full suite, 1,092 passed and 6 benchmark cases skipped; typecheck,
  scoped lint/format, build/bundle, template build, declaration/contract/artifact
  checks passed.
- Non-Effect: full suite after the package build, 1,089 passed and 26 skipped;
  package build/typecheck and scoped source lint/format passed. A first test run
  raced the build replacing `dist` and failed; the stable-artifact rerun passed.
- Added correctness cases pass against both the optimization and the exact
  baseline: transforms run on every Effect invocation, computed RegExp
  validation remains active, generic output semantics are retained, principal
  injection consumes no wire field, and nested streams retain lazy wrapping and
  single-owner transfer. Existing capability retry/ownership tests also passed.
- No AST evaluation, metadata-expression restriction, generated application
  codec, `eval`/`new Function`, packaging or preinitialization change is involved.
- These are **Node/V8**, not QuickJS or end-to-end Golem, measurements. No live
  Golem executable or Wasmtime runner was available in this orb; real-platform
  integration and QuickJS invocation benchmarks remain unverified. The Effect
  guest template was rebuilt, but not executed on Golem.
- Generic value trees are still allocated. This small change removes wrapper
  preparation and one resource-free async traversal, not all intermediate
  allocations. Resource-bearing and unsupported-direct shapes keep the existing
  asynchronous path; their performance is not measured here.
