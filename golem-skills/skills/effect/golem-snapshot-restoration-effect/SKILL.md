---
name: golem-snapshot-restoration-effect
description: Configures durable snapshots and restoration for Effect Golem agents. Use for recovery, schema state, custom bytes, or SQLite restoration.
---

# Snapshot restoration

Set `snapshotting: Snapshot.define({ schema, policy })` and use
`.implement({ init, methods, snapshot })`. `init(id)` exclusively infers runtime state. For a common
`Ref<State>` whose saved value matches the schema, use `snapshot: Snapshot.ref<Saved>()`. For a custom
representation use `{ save: state => Effect<Saved>, restore: (saved, context) => Effect<State> }`.
When SQLite databases are declared, add `databases: state => ({ declaredName: state.handle })` to
that strategy. The runtime restores database images before constructing methods.

Use `Snapshot.custom({ policy })` with implementation `save`/`restore` only for user-managed bytes.
Ordinary agents omit `snapshot`. External PostgreSQL, MySQL, and Ignite state is not included.

Use host-backed `Durability` combinators around external effects. Do not add compatibility migration
envelopes or legacy initialize/restore factories.
