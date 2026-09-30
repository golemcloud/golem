# GOL-40 MoonBit Chunk B conformance fixture

`artifact.mbt` is the MoonBit authoring fixture for `GOL-40-CONTRACT-1`.
`tools_emit_test.mbt` checks its rich authoring IR and the generated provider/client projections.

## Existing proxy and reflection coverage

The executable `MoonBitToolReflectionCaller::round_trip` fixture in
`golem_sdk_example1/golem_moonbit_examples/tool_reflection.mbt` exercises reflection and a
definition-owned `ToolClientDefinition` proxy for the same encoded call and structured result.
`optional_round_trip` compares reflected and ordinary generated-client optional-input encoding,
while `canonical_round_trip` covers canonical 64-bit, duration, and quantity values. The GOL-40
artifact assertion verifies both generated constructors (`new` and proxy-targeted `new_for`), nested
subclients, canonical inherited-argument order, typed errors, principal injection, and
stdin/stdout/stderr surfaces.

Principal and owner observations and complete stream tuples need a deployed host. They remain work
for the executor matrix rather than being represented by an SDK echo or a source-text-only runtime
claim.

Several fields cannot currently be emitted exactly by MoonBit's public derive surface and are therefore
kept as explicit blockers rather than normalized differences:

- `#derive.tool` has no alias property and a namespace-only root has no command method on which to
  place `#derive.command(alias=...)`, so the root `art` alias cannot be emitted;
- namespace-only root globals (`region` and `trace`) can only be attached to the `render` subtree
  mount; the wire descriptor therefore places them on `render`, not the namespace-only root;
- the SDK has schema-model support for constrained `text`, but no user-facing value wrapper, so
  `ArtifactReport.digest` is emitted as `string` rather than constrained `text`.
- MoonBit enum constructors must start uppercase and schema derives preserve constructor spelling,
  so `ArtifactStatus` emits `Queued | Ready | Failed`, not the lowercase contract cases.
- typed error cases accept at most one MoonBit payload, so `render-failed` uses the named
  `RenderFailure` record instead of the contract's inline record.
- provider stream parameters are always emitted with `required = true`; the fixture cannot express
  CONTRACT-1's optional stdin and stderr declarations.

## Existing terminal coverage mapped for SDK-TERMINALS

The provider lifecycle oracle remains
`golem_sdk/tool/provider_stdout_wbtest.mbt`; it is deliberately not copied into this fixture.

| Required terminal cell | Existing MoonBit test |
| --- | --- |
| partial bytes + success / declared error | `invocation stdout terminals preserve structured results and original errors` plus `completion drains every chunk of an already started write all` |
| explicit stdout failure + structured success | `invocation stdout terminals preserve structured results and original errors` (`explicit-failure`) |
| exception | same matrix (`exception`) |
| explicit invocation cancellation | `real task cancellation during invocation and completion releases stdout and discarded result` |
| writer abandonment | same matrix (`abandoned`) and `invocation completion and drop wait for an outstanding writer operation` |
| finish failure after success / declared error | same matrix (`finish-error`, `finish-error-declared`) |
| explicit client invocation cancellation | `client_stream_cancellation_wbtest.mbt` (`explicit invocation cancellation is distinct from observer and output drop`) |
| output-reader cancellation | `client_stream_cancellation_wbtest.mbt` (`cancelling a blocked output read preserves the result observer`) |
| dropped result observer | `client_stream_cancellation_wbtest.mbt` (`dropping a raw result observer does not cancel its output reader`) |

The matrix independently asserts the structured result/error, terminal transition, and exactly-once
provider resource drop. The client cancellation tests independently assert stream/result resource
cleanup. Runtime owner outcome and provider-observed owner identity require the later executor
matrix; they are not observable in SDK-only tests.
