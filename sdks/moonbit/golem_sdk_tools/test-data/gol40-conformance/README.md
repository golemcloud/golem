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

The fixture uses the public MoonBit authoring surface for the complete CONTRACT-1 metadata:
namespace-root aliases and globals, a directly grafted executable subtree root, constrained text,
authored enum case names, path extensions, complete argument/result/formatter documentation,
inline error-record payloads, stable schema display names, and independently optional stdin/stderr
declarations. The metadata assertion compares the generated descriptor projection exactly; it does
not normalize language-specific differences or use legacy metadata carriers.

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
