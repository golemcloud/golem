# Operations

A Memory combines retrieval, optional model work and storage. A replacement Memory keeps the API and
the guarantees on this page. It does not have to keep the same facts, rankings or answers.

## Values

- **Observation:** an observation ID, ordered text parts, source references and the original caller.
  The wrapper converts Golem AI messages and tool outcomes into text parts. Explicit notes use the
  same input. Binary content stays a reference unless code supplies text.
- **Memory item:** an opaque reference, text and the IDs of its supporting observations. A derived
  item keeps its sources. A composite item also keeps the names of every member that returned it.
  Backend scores stay private, because their scales differ.
- **Context:** ordered items and the measured token use of their rendering.

## Signatures

```text
observe(observation) -> ObservationId
recall({ query, limit, retrieval? }) -> MemoryItem[]
context({ query, tokenBudget, measureTokens, retrieval? }) -> Context
forget({ references | observationIds }) -> Unit
tools(namespace = "memory") -> LocalTool[]
```

These signatures define behavior. Each SDK uses its own idiom for them. `query` is text, not a
database query. There is no `text` alias.

| Operation | Behavior |
| --- | --- |
| `observe` | Stores the source, then runs the configured enrichment. See [Enrichment and derivation](enrichment.md). |
| `recall` | Returns ordered evidence items, not a generated answer. A read inserts no facts and runs no enrichment. |
| `context` | Selects candidates and packs complete items within the budget. An item that does not fit is skipped and later items are still considered. Evidence is never truncated, and a read needs no summarizer. |
| `forget` | Removes the selected items and the summaries, vectors and facts that depend on them. See [Forgetting](#forgetting). |
| `tools` | Returns local tools for recall, note insertion and forgetting. |

`limit` and `tokenBudget` are nonnegative integers. A zero value returns an empty result and calls no
model (`retrieval/zero-limit-and-budget-call-no-model`).

## Errors

The fixtures use these error kinds. Each SDK chooses its own types and names. It keeps the
distinction between the kinds and keeps the cause.

| Kind | Meaning |
| --- | --- |
| `input` | The request is invalid, such as different content under a recorded observation ID or an invalid selector. |
| `limit` | A finite limit is exhausted. The error names the limit. It is never an empty result. |
| `storage` | A database or file operation failed. |
| `authority` | File or tool authority denied the operation. |
| `retrieval-model` | The query embedding failed. It is never an empty result. |
| `arbiter` | The arbiter's classifier failed or returned malformed output. |
| `invalid-selection` | The arbiter's classifier selected a member that has no candidate result. |

A composite error names the member that failed. Native cancellation is not an error kind: it
propagates in the SDK's cancellation idiom, and no Memory converts it into another error, a
diagnostic or an empty success.

## Identity and repair

Observation IDs are opaque UUIDs within the owning agent.

- The entry Memory generates an absent ID. A composite generates it once and sends the same ID to
  every member (`identity/composite-allocates-once`).
- Identical ordered parts and source references under a recorded ID keep the original source and
  provenance. A later caller does not overwrite the original caller
  (`identity/same-id-same-content-keeps-original`).
- Different parts or source references under a recorded ID are an `input` error that changes nothing
  (`identity/same-id-different-content-conflicts`).
- Equal text under different IDs is different evidence (`identity/equal-text-distinct-ids`).
- Repeating a composite `observe` with the same ID and content repairs only the members that missed
  the write, without duplicates (`repair/partial-product-write`). Missing enrichment is considered
  again, but its success is not guaranteed.

Each backend enforces these rules locally. Composition adds no identity service or retry ledger.

## Cancellation and provenance

Every operation and every wrapped-model call receives the cancellation of the current invocation.
A reusable Memory, model or tool descriptor never keeps the signal of its first invocation. Two
overlapping calls on one Memory keep their own callers and signals; cancelling one does not cancel the
other (`cancellation/overlapping-calls-keep-their-own-signals`). Cancellation does not roll back
committed effects: a source that committed before cancellation stays.

The host supplies the original caller of the invocation. A Memory records it with each observation as
the caller's pair text, or as anonymous. Provenance holds no token, credential or claim set, and it
grants no authority, read isolation or forget-by-caller selector. Caller identity written inside the
observation content is not provenance.

## Retrieval

Retrieval is `lexical` or `hybrid`. The default is `hybrid` when the Memory has an embedder, and
`lexical` otherwise. A lexical read embeds no query. A failed query embedding is a `retrieval-model`
error (`retrieval/query-embedding-failure-is-an-error`).

## Context budgets

`context` and `wrap` require a caller-supplied `measureTokens` function. It measures the rendered
Memory context, including separators and citations. The budget bounds that rendered context only. It
is not a bound on the whole prompt and not a provider billing guarantee. It does not bound scans,
embeddings or classifier input; [Limits](limits.md) bound that work.

Packing adds candidates in order. It adds a candidate only when the rendering of the chosen items and
the candidate still measures within the budget (`context/complete-items-skip-oversized`).

## Forgetting

`forget` takes exact references or observation IDs, exactly one of the two, as a nonempty list. It
never takes a query that a model interprets.

- **Observation IDs** remove the sources and everything derived from them. A shared fact loses that
  source's support and survives while another retained source supports it
  (`forget/observation-removes-support`).
- **References** remove the referenced items. Removing a derived item keeps its source, which can
  support a later derivation (`forget/derived-item-leaves-its-source`). A reference from another
  Memory is an `input` error (`forget/invalid-selectors`). A composite reference targets only its
  recorded member origins (`forget/reference-targets-recorded-origins`).

Whether a reference to a literal source item can delete the source is open. The fixtures do not use
one, and the reference memory rejects it.

## Tools

`tools` returns local tools for recall, note insertion and forgetting. Reads return references that
the forget tool can target. The default namespace is `memory`; an independent Memory can choose
another one. Assembling two toolsets with the same tool names fails; nothing is overwritten or renamed
(`tools/namespace-collision`). A namespace does not select a database identity or a hook name. The
application passes the semantic tools explicitly; database tools and their grants stay separate.

The tool names and schemas follow the Golem AI local tool contract and remain open here.

## Constructor paths

A built-in Memory receives its mutable database location as a writable constructor path with a
conventional default and an ordinary string config override. It is not a deployed read-only `File`
field. Independent memories never share storage silently. A path grants no authority. Exact path
spelling, name derivation and collision handling remain open.
