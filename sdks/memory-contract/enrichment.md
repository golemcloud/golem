# Enrichment and derivation

Enrichment is optional model work that `observe` runs after it stores a source: a summary, an
embedding or extracted facts. Its result is a derived result. Reads never run enrichment.

## Failures

- A summarizer, extractor or embedder failure that is not cancellation leaves the source intact and
  emits an SDK diagnostic. `observe` still succeeds
  (`enrichment/failure-keeps-source-and-reads-do-not-retry`).
- Malformed model output, or output that cites an unknown source part, is a failed derivation with a
  diagnostic, not a successful empty derivation (`derivation/malformed-output-is-a-failed-derivation`).
- A successful empty derivation is complete work. It is recorded and never retried
  (`derivation/successful-empty-is-complete`).
- Cancellation, input, database and file-authority failures propagate. Cancellation propagates before
  any optional-enrichment handling. It is never a diagnostic or an empty derivation
  (`cancellation/is-not-an-enrichment-diagnostic`).

## Retry of missing enrichment

A source with no derived result of a configured kind has missing work. A later `observe` first
retries the missing enrichment of at most one earlier retained source that has the same identified
original caller, then processes its own observation (`derivation/same-identified-caller-retry`,
`derivation/at-most-one-earlier-source`).

- Anonymous sources never match across invocations, so no anonymous observation retries another
  (`derivation/anonymous-sources-do-not-match`).
- The retry uses the current invocation's authority, model, quota and cancellation. Provenance
  restores no earlier authority.
- A caught model failure is not retried only because execution is durable. An idle agent has no
  retry-time guarantee. There are no public pending jobs, receipts or scheduler.

## First-valid commits

Model calls run outside write transactions and connection locks. When a model call returns, the
Memory checks in one protected transaction that the source still exists and that no result of the
same derivation identity exists, and inserts the result only then.

- For one derivation identity the first valid commit wins. A later equivalent result neither
  overwrites it nor adds a duplicate (`derivation/first-valid-commit-wins`).
- Overlapping model calls, and their costs, remain possible. This is not exactly-once model
  execution or request coalescing.
- A source that is forgotten during model work loses its support. The late result is skipped and
  nothing returns (`derivation/forget-during-model-work`).
- Deleting a derived item keeps its source. A later observation can derive the item again
  (`forget/derived-item-leaves-its-source`).

The derivation identity distinguishes the source, the derivation kind and the relevant configuration,
such as the prompt identity or the embedder identity. The exact key remains open. A configuration
change does not require reprocessing every earlier source.

## Embedded connections

An embedded adapter shares one SDK lock per database connection. It takes the lock before `BEGIN`
and does not suspend between `BEGIN` and `COMMIT` or rollback. It releases the lock before a model
call. This coordinates the users of one connection only; it does not coordinate separate guest
connections or a guest connection and a tool.
