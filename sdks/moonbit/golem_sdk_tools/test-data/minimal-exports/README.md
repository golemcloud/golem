# Same-world minimal exports

From `sdks/moonbit/golem_sdk`:

```sh
python3 scripts/check-minimal-exports.py --output /tmp/minimal-exports
```

Requires `moon`, Node, and `wasm-tools`. The output directory is disposable: the
script recreates its four fixture package directories. It combines these source
files into empty, tool-only, agent-only, and mixed packages, runs both codegen
commands, checks regeneration idempotency, builds release WASM with names,
strips all custom sections, embeds the unchanged full SDK world, and validates
the resulting components. No role or alternate world is selected.

The Node harness checks the exact complete core export list against `gen/moon.pkg`,
executes discovery and snapshot save through canonical ABI exports, and invokes
the tool's `ping` and `burst` methods through the asynchronous canonical ABI.
`burst` verifies every byte across sizes 0, 1, 63, 64, 65, 4096, and 65537; records
provider write calls; and checks exactly one finish and drop. These boundaries
exercise `ProviderStdout`'s 64 KiB callback window using the generated async runtime's
bounded-copy API. The expected seven writes comprise five small chunks and two
chunks for 65537 bytes; the empty write is skipped. This is a tool stdout
`write(list<u8>)` probe, not a direct component `stream<u8>` probe or a count of
Wasmtime host reader calls, which may coalesce submissions. The MoonBit tests
also exercise generated registration, SDK guest dispatch, agent initialization,
state mutation, JSON snapshot envelopes, already-initialized snapshot rejection,
and explicit no-agent/no-tool errors.

`measurements.json` records release-with-names, stripped core, and component sizes;
code-section bytes grouped by owning package using MoonBit's mangled symbol names;
and warmed median latency. Function-body attribution excludes data, name-section
bytes, and function headers. Inlined code is attributed to its caller. Latency
includes JS allocation/encoding/assertions and is **not** executor or network
latency. This harness does not supply real guest-to-host tool RPC or snapshot
restoration's `parse-agent-id` host; run platform integration tests for those.

For before/after generator comparisons, retain separate output directories, update
the pinned wit-bindgen revision, regenerate only through
`golem_sdk/scripts/regen-bindings.sh`, and rerun this command. Do not edit generated
bindings manually. `--sdk <other-checkout>/sdks/moonbit/golem_sdk --measure-only`
measures an older SDK using its own code generator; `--tools` overrides that path.
Compare identical sources, compiler versions, and consecutive runs. The stream
counts are deterministic; wall-clock timings are sensitive to orb load.
