# Region-simple async-trait implementations

This internal attribute runs **after** `async_trait` (including its `tonic`
re-export). Write ordinary async methods:

```rust,ignore
#[async_trait::async_trait]
#[golem_async_trait::region_simple]
impl Operation for Subject {
    async fn run(&self) { /* ordinary body */ }
}
```

The public expanded signature stays unchanged. Its wrapper forwards to a private
inherent helper whose construction borrows share one lifetime. The helper keeps
the existing `Box::pin(async move { ... })`: construction stays lazy, cancellation
drops the same captures, and the returned future still has one allocation.
`Send` and `?Send` bounds are not asserted or changed. Instrumented methods retain
their original span name (default, named, or positional) and body instrumentation;
trait lookup imports stay inside the helper's async block.

## Bounded support

Use this only on selected implementations, not every async-trait implementation.
The self type must permit an inherent implementation. Implementation parameters
must be constrained by that self type, not only by the trait path. Default trait
bodies are unchanged. Qualify inherited associated types at the source when they
would not resolve in an inherent implementation.

Source methods with explicit lifetimes are excluded. The macro rejects
distinguishable explicit lifetimes, but cannot distinguish an explicit `life0`
from async-trait's generated `life0` after expansion. Source selection must check
that condition. Implementations with wildcard parameters (`_: T`) are also
excluded: the expansion leaves no identifier to forward. The implementation
lifetime `__region_capture` is reserved and rejected. Unsupported boxing
expansions and incorrect attribute order fail with a diagnostic rather than a
different transformation.

Conditional `instrument` attributes inside `cfg_attr`, including nested
conditions, are rejected. Use a direct `instrument` attribute instead; duplicating
conditional instrumentation on the wrapper and helper would create extra spans
and instrument future construction rather than only polling.

Helper names include a fingerprint of the qualified trait path and its generic
arguments. They remain private implementation details. The macro uses workspace
Syn 2 and the existing async-trait dependency without modifying either.

The contract tests cover public dispatch, borrows and generics, associated types,
`Send`/`?Send`, lazy execution and drop, tracing order and names, scoped trait
lookup, and qualified-trait/generic-instantiation name separation. A thread-local
allocation counter checks that construction, polling, and drop together allocate
once, matching plain `async_trait`, without counting other parallel tests.
Expansion comparison provides additional validation outside these tests.
