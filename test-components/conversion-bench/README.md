# Invocation conversion fixtures

Five SDK implementations expose the same two ordered-record methods:

* `checksum(input: List<U8>) -> U32`: wrapping sum, independently checked by the
  harness with complete 251-byte periods plus a triangular remainder.
* `produce(length: U32) -> List<U8>`: deterministic bytes `i % 251`.

The TypeScript typed array marker is List<U8>, **not** Binary. MoonBit uses
Array<Byte>, not Bytes. Input and output are measured independently; echo does
not substitute for either. Guest construction and durable invocation semantics
are unchanged. These methods do not validate stream/resource optimizations;
use the workers' ownership/cancellation fixtures for those cases.

Build the local SDK bundles/base runtimes first, following their repository
guidance. Scala needs JDK21 and local SDK/plugin publication. MoonBit needs the
pinned Golem wit-bindgen and regenerated SDK bindings. Then, in this directory:

```sh
GOLEM_TS_PACKAGES_PATH=../../sdks/ts/packages golem-cli build --yes
golem-cli --yes exec copy
```

The `copy` commands put only these five WASMs in `test-components/`. From the root:

```sh
cargo run --profile benchmarks -p integration-tests --bin benchmarks -- \
  suite --check-artifacts integration-tests/benchmark_suites/conversion.yaml spawned
cargo run --profile benchmarks -p integration-tests --bin benchmarks -- \
  --retain-details suite integration-tests/benchmark_suites/conversion.yaml spawned
```

Use the same harness, fixture source/locks, build profile, toolchain, instrumentation
and hardware for baseline/current. Save service/fixture hashes and source revisions
with results. Each SDK runs sequentially; `size` workers run concurrently. Three
warmups precede nine samples per worker per iteration. `client-total` includes
native preparation, cloning for retries, JSON encoding, transport and response
decoding. Recovered calls are separate and failures remain visible. Do not remove
client work from this series or compare it to historical per-invocation numbers.
The historical Rust/TS REST, mapped HTTP and local/remote RPC suites remain separate.

The test-framework owned-value consumption change is harness-only. Keep it
identical on both sides of production comparisons and report it separately.
These REST cases do not measure generated external Effect wrapper costs; run the
generated bridge checks and separate SDK/runtime codec measurements as well.
