# Opt-in invocation codec measurements

Run sequentially, without other builds or benchmarks:

```sh
cargo run --locked --release -p golem-codec-measurements > tmp/codec-baseline.jsonl
```

Run the same harness commit on the reviewed production baseline and integrated
production checkout. Keep Cargo.lock, rustc, target, profile, hardware and
instrumentation identical. Record those revisions and `sha256sum Cargo.lock`
alongside the raw JSONL. Compare each stage independently; do not add their gains.
This counts global allocator calls, requested bytes and incremental live-byte peak,
not RSS, allocator fragmentation or uninstrumented production wall time. Realloc
counts as one allocation and its requested size. Counter overhead is included in
every sample. Each stage has three warmups and nine raw samples, median and range.

Inputs are ordered records containing List<U8>, not Binary. Asymmetric bytes
exercise the same shapes at lengths 100 and 10,000. Roundtrips and DOM equality
are checked before measurement. Codec timings are neither guest runtime timings
nor evidence that the end-to-end regression is fixed. Construction/preflight,
encoding, decoding and harness-only clones are separate series. Client work must
remain inside existing invocation timers when running throughput comparisons.

`external-decode-dom-including-parse` deliberately includes parsing so owned DOM
consumption does not appear faster merely by shifting parse work outside the timer.
The existing CI benchmark suite and its timer boundaries are unchanged.

Pass `-- --zero` to reproduce the original all-zero root record separately
(240,077 JSON bytes at 10,000 elements). Keep the default asymmetric comparison.
`json-dom-parse-only` isolates JSON parsing from tagged-value decoding; subtracting
medians is only an attribution estimate, not an independently timed decoder.

Add `--features transports` for a separate instrumented protobuf breakdown:
native clone/lower, protobuf clone, encoded_len, encoding, decoding and clone/lift.
These measure the current List<U8> representation, not a hypothetical packed
carrier. Preserve the same feature set within each comparison.
