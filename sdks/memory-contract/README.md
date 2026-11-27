# Memory contract

This directory owns the language-neutral Memory contract and its canonical fixtures. Every SDK
implements the contract in its own idiom and ports the fixtures to its own tests. The
[Memory development plan](https://linear.app/golem-cloud/document/development-plan-memory-9c14de5ee804)
selects the design; these pages state the parts that implementations and fixtures share.

No SDK ships a Memory API yet. The contract is the input for the SDK core, access and backend tasks.

## Pages

- [Operations](operations.md): observations, items, the five operations, errors and selectors,
  identity and repair, cancellation and provenance, budgets and tools.
- [Enrichment and derivation](enrichment.md): optional model work, retries, first-valid commits
  and forgetting during model work.
- [Composition](composition.md): product, fallback and arbiter, with examples.
- [Limits](limits.md): the finite defaults and their boundaries.
- [Prompts](prompts.md): the default prompts and output schemas.
- [Storage](storage.md): values, the SQLite profile, query lifecycle, hooks and package cards.
- [Porting the fixtures](porting.md): the fixture format and how an SDK runs it.

## Fixtures

| File | Content | Consumers |
| --- | --- | --- |
| `fixtures/semantic-v1.json` | Operation scripts against custom memories and every composition, with expected results, member state and traces | SDK core and composition tasks; backend tasks for the backend-independent cases |
| `fixtures/trace-v1.json` | The test-only trace vocabulary | Every consumer of `semantic-v1.json` |
| `fixtures/defaults-v1.json` | Finite defaults and their boundary cases | SDK core and backend tasks |
| `fixtures/prompts-v1.json` | Default prompts, output schemas, rendering cases and output cases | Backend and AI tasks |
| `fixtures/values-v1.json` | SQLite cells and batches; Cozo value expectations | SQLite and Cozo access tasks, ST-28 and ST-32 |
| `fixtures/storage-v1.json` | The SQLite profile, SQLite and Cozo workloads, refusal forms, lifecycle, hook and card scenarios | Access tasks, ME-02, ME-03, ST-28, ST-31 and ST-32 |

Each consumer compares its own independent results with these files. One SDK's output never becomes
another SDK's expected value.

## Self-check

```shell
node --test sdks/memory-contract/check.test.mjs
```

The check needs Node.js 24 and no dependencies. Main validation runs it in the
`main-validation-schema` task. It:

1. validates the fixture structure and cross-references;
2. runs the semantic cases through a test-only reference memory and the three composition forms in
   `reference-memory.mjs`, which shows that a custom memory and every composition can use the
   fixtures without a model or a database;
3. runs the SQLite value, batch, profile and workload cases on `node:sqlite`.

The expected values are written from the contract, not generated from the reference memory. The
reference memory is not an SDK implementation, a backend or a public API. Passing the check proves
nothing about an SDK, a Golem runtime artifact or a tool. Those tasks run the same fixtures on their
actual artifacts. The Cozo cases are checked only for structure; ME-03, ST-31 and ST-32 execute them.

When the contract changes, change the pages, the fixtures and the reference memory in the same
commit, and add a new fixture version only when consumers need both versions.

## Open items

These remain open. The fixtures do not select them:

- the exact derivation key of a derived result (source, kind, prompt or embedder identity);
- hook registration syntax and the supply of stable hook names;
- the renderer and counter signatures of `context` and `wrap`;
- the exact tool schemas of the `sqlite3` and `cozo` tools that carry SQLite cells and Cozo values;
- database path spelling, name derivation and collision handling;
- the exact local tool names and schemas, which follow the Golem AI local tool contract;
- the recovery error and repair workflow for a query that needs recovery.
