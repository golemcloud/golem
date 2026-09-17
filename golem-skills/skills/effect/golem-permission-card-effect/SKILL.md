---
name: golem-permission-card-effect
description: Transfers opaque permission cards across Effect agent, tool, and middleware boundaries. Use when delegated authority is part of a schema.
---

# Permission-card transfer

Declare a card with `Schema.PermissionCard({ polymorphic: false })` from `@golemcloud/effect-golem`. The resulting value is an opaque affine handle: pass it through an agent method, tool call, or middleware result, but never inspect, clone, stringify, persist, or reuse it after successful transfer.

Place the card directly in the input/output schema. Encoding consumes ownership. If encoding a composite value fails, the SDK restores adopted capabilities so the operation can be retried. Do not add wallet, derivation, or installation APIs; they are not public Effect SDK capabilities.
