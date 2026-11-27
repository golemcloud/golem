# Porting the fixtures

An SDK task ports the fixtures into its own tests. It reads the files from
`sdks/memory-contract/fixtures/` or embeds them with a check against drift, as other cross-SDK
corpora do. Expected values always come from the fixture files, never from another SDK.

## Semantic cases

Each case of `semantic-v1.json` builds scripted members and composites, runs steps and compares the
results with the expectations.

- `ids` maps `@` aliases to observation IDs. `callers` maps caller aliases to pair text. A step
  without a caller runs as an anonymous caller.
- `members` are scripted custom memories, which are test devices, not backends:
  - `items` are preset knowledge items with sources.
  - A source is also an item; its text is its parts joined with a newline.
  - Candidates are the items whose words match a query word, case-insensitively. The order is preset
    items, then sources, then derived summaries, each in insertion order.
  - `summarizer` enables one summary per source.
  - `queryEmbedder` enables hybrid retrieval with query embedding only; a scripted member embeds no
    sources.
  - `faults` fail the n-th call of an operation with an error kind.
  - `limits` override defaults.
- `memories` builds composites in declared order. A composite can use an earlier composite as a member.
- `models` script each model role: `summarizer`, `embedder` and `classifier`. Entry n answers call n of
  that role. `{ "gate": g, "then": entry }` waits until a `release` step opens `g`, or until the call's
  signal is cancelled.
- Steps:
  - An `op` step runs one operation and waits for it.
  - `start` runs one without waiting, and `await` waits for a started one.
  - `cancel` cancels a named signal, and `release` opens a gate.
  - `as` stores a result, and `{ "result": name, "index": i }` references a stored item.
  - `{ "capture": name }` stores a returned observation ID as `$name`.
  - After `start`, `cancel` and `release`, let every pending operation run until it waits again.
- The fixture token counter, which `context` steps pass as `measureTokens`, counts `#` characters.
- `expectState` compares member state; `expectTrace` compares the complete ordered list of trace
  events (`trace-v1.json`); `expectModelCalls` compares call counts per role; `expectConstruction`
  compares composite construction.
- An error expectation compares the error kind and, when given, the member and the limit. Absent
  fields must be absent.

Only test devices emit trace events. The trace vocabulary is not a public Memory API. An SDK may
represent the scripted members and models in its own idiom, but it must not change an expectation.

## What a port proves

A port that passes the semantic cases with the SDK's real composition code and a custom memory shows
that the SDK's core API supports the contract without AI or a database. It does not prove storage,
real overlapping invocations, restart, replay, hooks or tool behavior. The backend, access and final
tasks run those on actual artifacts, using `values-v1.json`, `storage-v1.json` and the backend-
independent semantic cases against the real backends.

The `reference-memory.mjs` module of the self-check is one such port. It is a test device, not an
SDK implementation.

## Values and storage

- Bind each SQLite cell's `bind` value with the SDK's typed API, read it back and compare with `read`,
  including the storage class.
- Run each batch as one atomic batch and compare the remaining rows.
- Run each workload with the connection profile applied.
- Run the refusal forms and lifecycle scenarios against the query command of the actual tool or
  embedded route.
- Cozo cases compare the decoded value with `expect`. A case passes only when the tool schema keeps the
  named distinction.
