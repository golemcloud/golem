# Composition

A composite is a Memory whose members are Memories. Members have unique names and a declared order.
A composite can be a member of another composite. Every form:

- sends each observation to every member in order, with one observation ID;
- sends observation-ID forgetting to every member in order;
- sends a reference only to the member origins that the reference records;
- runs writes in member order and stops at the first failure. The error names the member; the earlier
  members' writes remain;
- exposes one toolset through its own operations. Note insertion is a composite `observe`. The
  composite never unions the raw member tools, which could bypass its rules
  (`tools/composite-tools-use-composite-operations`).

A composite promises no transaction across members. Success of a write means that every targeted
member completed. A failed `forget` does not claim absence from any member
(`forget/failure-names-the-member`). Repeating the same observation repairs the members that missed
it ([Operations](operations.md#identity-and-repair)).

## Forms

| Form | `recall` | `context` |
| --- | --- | --- |
| Product | Reads every member up to the requested limit. Interleaves the results in member order. Merges items with identical text and identical sources, and keeps every origin. Applies the final limit last. | Asks every member for context within the full budget. Interleaves and merges the items, then packs them under one final budget. Makes no model call. |
| Fallback | Reads members in order and returns the first nonempty result. Empty is the only fallback condition. Errors propagate. | Returns the first nonempty member context. Errors propagate. |
| Arbiter | Reads every member. Returns empty when all are empty, and a sole nonempty result directly. Otherwise a classifier selects one named member's result. Merges and rewrites nothing. | Makes the same choice between member contexts. |

Cases: `product/recall-interleaves-and-deduplicates`, `product/context-packs-under-one-budget`,
`fallback/empty-falls-through`, `fallback/error-propagates`, `arbiter/no-classifier-without-a-choice`,
`arbiter/classifier-selects-one-member` and `arbiter/invalid-selection-and-classifier-failure`.

The arbiter receives a classifier function. A classifier failure or malformed output is an `arbiter`
error. A selection that names no candidate is an `invalid-selection` error. The optional Golem AI
helper calls an unwrapped configured `Llm` in exact form with the [arbiter prompt](prompts.md). It is
the only composition that needs a model.

## Shared limits

A composite has its own finite limits. Its members share the remaining candidate rows and candidate
bytes of one operation; exhaustion is a `limit` error that names the member
(`limits/shared-candidate-rows`). Member fan-out counts every member of the composite tree, nested
composites and their members included (`construction/member-fan-out-counts-nested-members`).

## Examples

A custom memory needs no model and no database. It implements the five operations:

```text
profile = custom memory {
  observe(observation)  -> store the parts that name a preference; return the observation ID
  recall(query, limit)  -> the stored preferences that match the query, with their sources
  context(query, budget, measureTokens) -> pack the recall result within the budget
  forget(selector)      -> remove the selected preferences
  tools(namespace)      -> recall, note and forget tools over these operations
}
```

Mira owns Atlas, Atlas depends on Orion, and Orion's delivery moved to Friday. A SQLite memory can
return the original delivery passage. A graph memory can return the Mira–Atlas–Orion path. Their
product returns both, each with its sources and origin:

```text
memory = product(sqlite: sqliteMemory, graph: graphMemory, profile: profile)
items  = memory.recall({ query: "who needs to know about the Orion delay", limit: 5 })
memory.forget({ observationIds: [items[0].sources[0]] })   // removes that source in every member
```

A fallback uses the profile only when the conversation memory has nothing:

```text
memory = fallback(conversation: sqliteMemory, profile: profile)
```

Test each member's contribution, not identical retrieval lists.
