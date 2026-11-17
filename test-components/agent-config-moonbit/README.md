# MoonBit static configuration guest

`StaticDefaultsAgent(String)` has the reference declarations from
`test-data/config-conformance/v1.json`: a named record through an alias, a custom
discriminated union, a tuple with two optional positions and an optional string
without a default. `StaticShapeAgent()` adds a recursive named record, a renamed
field, explicit null, false and an empty list. Both keep the injected `Config`
and return the captured settings from `read`.

Use the MoonBit and Golem wit-bindgen versions pinned by `sdks/moonbit/AGENTS.md`
and `golem_sdk/scripts/install-wit-bindgen.sh`. The local workspace resolves the
SDK and generator from this checkout. This fixture is not in the broad component
build list; build it explicitly before a consuming host test.

From `sdks/moonbit/golem_sdk_tools`:

```sh
moon run cmd -- reexports ../golem_sdk ../../../test-components/agent-config-moonbit/component
moon run cmd -- agents ../../../test-components/agent-config-moonbit --component-dir component
```

From this directory:

```sh
../../sdks/moonbit/golem_sdk/scripts/run-sdk-tests.sh component/config_test.mbt
moon check --target wasm component
moon build --target wasm --release component
node check-core.mjs _build/wasm/release/build/golemcloud/agent_config_moonbit/component/component.wasm
wasm-tools component embed ../../sdks/moonbit/golem_sdk/wit \
  _build/wasm/release/build/golemcloud/agent_config_moonbit/component/component.wasm \
  --world agent-guest --encoding utf16 \
  --output _build/wasm/release/agent_config_moonbit.embed.wasm
wasm-tools component new _build/wasm/release/agent_config_moonbit.embed.wasm \
  --output _build/wasm/release/agent_config_moonbit.agent.wasm
wasm-tools validate --features all _build/wasm/release/agent_config_moonbit.agent.wasm
```

`check-core.mjs` executes actual metadata, initialization and invocation exports.
It compares declarations and schema graphs with the independent corpus and gives
the host import the corpus's already-resolved expected values. It checks capture,
unchanged reads after a host-value change, fresh-instance recapture, canonical
paths, recursive data and failed host reads/decodes without a code fallback.
It does **not** resolve selectors, merge layers, validate supplied config defaults
or replay an oplog. Fresh instantiation is not executor reconstruction.

The generated-override test checks that recursive creation values use canonical
config field names while ordinary method schemas retain native field names.

## Hosted acceptance

Use the final component above with the existing
`static_config_defaults_and_creation_overrides_survive_reconstruction` host case.
Its `StaticDefaultsAgent`, constructor string, `read` result and config paths match
the corpus reference. Register this component under an `agent_config_moonbit`
`PrecompiledComponent` fixture, then reuse that case's deployment/creation layers,
both reference reads, executor shutdown/restart and settled-oplog checks. The host
test must consume the prebuilt component rather than launch MoonBit or Node.
The SDK core-export check alone does not complete this hosted acceptance.
