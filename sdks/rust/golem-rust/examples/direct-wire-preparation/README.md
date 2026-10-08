# Direct-WIT guest preparation measurement

This opt-in command measures the actual `IntoWire::prepare_wire` and
`encode_async` implementations in an optimized WASI guest. The standalone
manifest avoids the SDK example target's host-only test dependencies. Its
release profile matches the Rust SDK (`opt-level = 's'`, LTO, stripped symbols).
The lockfile retains the SDK's dependency revisions.

Build and run from the repository root, with Wasmtime 46.0.1 on `PATH`:

```sh
cargo build --locked \
  --manifest-path sdks/rust/golem-rust/examples/direct-wire-preparation/Cargo.toml \
  --release --target wasm32-wasip1
wasmtime run \
  --preload 'golem:core/types@2.0.0=sdks/rust/golem-rust/examples/direct-wire-preparation/resource-free-host.wat' \
  sdks/rust/golem-rust/examples/direct-wire-preparation/target/wasm32-wasip1/release/direct-wire-preparation.wasm
```

For allocation observations, prefix the build command with
`GOL720_COUNT_ALLOCATIONS=1`. Unset that variable and rebuild for normal-allocator
timings. The compile-time choice is printed to stderr. CSV allocation columns
are **not measured** in the normal-allocator build, rather than observed zeros.
Counts include reallocations and cumulative requested bytes, not peak memory.

Counting allocator side effects can prevent allocation elimination. In
particular, flat byte-vector preparation can optimize away in isolation without
instrumentation even when the counted build allocates a future for each byte.
Do not infer production allocations or performance from that count alone.
Compare uninstrumented timings too, including nested vectors and complete
encoding, whose optimization context differs from isolated preparation.

Each case uses ten warm-up calls and seven timed samples. Preparation samples
contain 1,000 calls; encoding samples contain 100 calls. Inputs contain 100 or
10,000 bytes with the deterministic `i % 251` pattern. Nested inputs contain
ten-byte chunks. Both encodings are checked by decoding back to the independent
input before measurement. Encoding retains a node-count assertion in the timer.
Run baseline and changed binaries alternately on the same machine without
competing builds, and report medians and variation, not timing assertions.

The preload module traps if any capability is dropped. These resource-free
cases retain resource drop imports through wire-enum drop glue, but must never
invoke them. This is a core WASI guest codec measurement, not a Golem invocation
benchmark or a measurement of resource-bearing calls.
