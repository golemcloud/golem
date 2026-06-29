Host interface for the Golem quota system.

Agents use quota-tokens to declare intent to consume a named resource and to
reserve / commit actual usage.

The `quota-token` capability itself is defined in `golem:core/types` so that
it can travel inside a `schema-value-tree` as an opaque, unforgeable handle.
This interface only exposes the operations that act on such a handle.