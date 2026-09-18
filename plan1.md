# GOL-476 Plan 1: Owner-Persisted Derived State

## Summary

Keep general and foreign readers read-only, move authoritative persistence to worker-owner
initialization, and order deletion so every failure leaves either an oplog-backed recoverable worker
or a final stale recovery-index entry that recovery can prune.

The steady-state performance impact should be small, but there are two significant degraded-path
risks: repeated status reconstruction after Redis cache loss, and repeated durable-stream suffix
folding on data-plane reads.

## Proposed ownership boundary

- Make `WorkerService::get_agent_mode` non-persisting; worker owners populate the durable mode key.
- Make generic `WorkerService::get` non-persisting while retaining reads from an existing cached
  status.
- Keep owner-side durable-stream index catch-up and persistence unchanged.
- Replace the foreign durable-stream probe's multiple independently persisting lookups with one
  combined read-only inspection.
- Make remote journal-lag telemetry use an existing covered projection and skip the sample when
  coverage is insufficient.
- On owner initialization, authoritatively reconcile status, assignment tracking, mode, and derived
  indexes before publishing the worker.

## Deletion and recovery

Delete in this order:

1. Capture the worker's mode and shard.
2. Delete all derived key-value state.
3. Delete the oplog.
4. Remove the worker from `RunningWorkers` last.

If derived-state deletion fails, the oplog and recovery membership remain, so retry or recovery can
reconstruct the worker. If only final membership removal fails, recovery acquires the relevant
lifecycle/service locks, checks both durable and ephemeral oplog namespaces directly, and prunes the
stale member. It must not infer oplog absence from `WorkerService::get == None`, because missing
component metadata can also produce that result.

## Reader performance analysis

### `WorkerService::get`

A call reads the mode, initial `Create` oplog entry, component metadata, and Redis status record. On
a status miss it reconstructs from a checkpoint or the oplog and currently writes the result back.

| Caller | Expected heat |
|---|---|
| Worker activation/recovery | Cold lifecycle path |
| Create/fork/promise/scheduler admission | Infrequent |
| Worker enumeration | Potentially hot and multiplied by workers per page |
| Public and RPC metadata operations | Potentially request-level |
| Guest metadata/existence/permission checks | Potentially invocation-level |
| Durable-stream consumer probe | Periodic and data-plane-adjacent |

Active workers normally bypass this through `ActiveAgents`, so it is not on the normal invocation
hot path.

With warm Redis, removing the write does not add work. The first cache miss becomes cheaper because
it skips status/index persistence. Repeated misses become potentially much more expensive.

Both live status and checkpoint are routed to Redis. After Redis loss, a dormant worker may have
neither. A full rebuild performs two oplog passes in 1,024-entry chunks:

```text
range reads ≈ 2 × ceil(oplog entries / 1024)
```

A one-million-entry worker therefore needs approximately 1,954 range reads per reconstruction,
excluding payload and reference reads. Repeated enumeration can pay that cost repeatedly because no
worker owner activates to repair the cache.

### `get_agent_mode`

On a warm call this performs one Redis read. On a miss it probes the durable and possibly ephemeral
oplog namespaces, then currently caches durable mode. Callers include every `WorkerService::get`,
paginated public and guest oplog reads, revert operations, durable-stream attachment checks, and
remote durable-stream lag inspection.

Making this non-persisting has a bounded cost: a dormant worker with a missing mode key repeats one
or two constant-time existence checks rather than scanning history. Ephemeral workers already behave
this way. Owner initialization should populate the durable mode key.

### Durable-stream indexes

The existing lookup sequence is:

```text
read oplog horizon
  -> read Redis coverage
  -> fold uncovered oplog suffix
  -> persist projection with CAS
  -> read requested fields
```

Owner-side callers include worker/session recovery, raw session reconstruction, producer metadata
loading, stream operations, and attachment catalogue/reconciliation state. Some are hot, so they
must retain persistent catch-up.

Foreign `DbDirectStreamAttachmentConsumerProbe` calls occur in:

1. Attachment reconciliation every 20 seconds, with batches of up to 256 attachments.
2. Stream-segment reads before serving data.
3. Blocking stream reads again after waiting.
4. Terminal journal comparison.
5. Remote journal-lag telemetry, potentially sampled every 100 ms.

The read-only replacement should capture mode and horizon once per distinct worker, read only the
required persisted projection fields, fold the uncovered suffix once in memory, and answer identity,
attachment-status, and journal-summary questions from that snapshot. Independent read-only versions
of every current lookup would risk folding the same suffix several times in one logical operation.

Telemetry-only journal-lag sampling should skip a sample when the persisted projection is behind
rather than reconstructing foreign state merely to produce a metric.

## Owner-initialization cost

An unconditional full reconciliation for an existing worker adds synchronous assignment tracking,
stream/invocation index coverage checks, Redis field listing, status persistence, and mode
persistence. It is not per invocation: it occurs on activation, eviction, restart, or reshard. It
can nevertheless create a significant burst during mass executor recovery.

A cache-provenance flag could avoid this write after a genuine persisted cache hit, but adds another
state distinction. Plan 1 initially favors unconditional reconciliation for simplicity, subject to
measuring recovery behavior.

## Performance validation before shipping

Measure:

1. Repeated enumeration after clearing Redis-derived status and checkpoint data while leaving
   workers dormant.
2. Mass worker recovery with unconditional owner reconciliation.
3. Stream-segment read latency with zero and nonzero stream-index coverage lag.
4. Reconciliation of 256 attachments with zero and 1,024-entry projection lag.

Use counting-storage tests to establish zero foreign writes, equivalent inspection results, and at
most one suffix traversal per distinct worker in one inspection.

## Open decision

Plan 1 accepts that, after Redis loss, frequently queried dormant workers remain expensive until an
owner activates them. If that degraded-mode behavior is unacceptable, owner-only persistence is not
enough and a safe cache-repair mechanism is required; restoring unguarded foreign writes would
reintroduce GOL-476.
