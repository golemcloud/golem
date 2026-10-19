---
name: golem-read-only-method-effect
description: Adds cacheable read-only methods to Effect Golem agents. Use for queries that do not mutate agent state.
---

# Read-only Effect methods

Set `readOnly` on a `method`; handlers must remain ordinary Effects and must not mutate state or perform writes.

```ts
value: method({ input: {}, success: Schema.Number, readOnly: true })
cached: method({
  input: {},
  success: Schema.String,
  readOnly: { cache: { ttlNanos: 5_000_000_000n } },
})
```

Whether a read-only result is cached per principal is derived from its input: declare a
`PrincipalSchema` parameter when the response depends on the caller. A read-only handler without
that declared parameter cannot access the ambient `Principal` service. Do not mark a method
read-only merely because its return type is immutable.
