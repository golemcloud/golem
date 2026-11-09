# Oplog API

> `OplogApi` — the native Kotlin/Wasm binding to Golem's oplog **read** surface `golem:api/oplog` (`get-oplog` /
> `search-oplog`, package version `1.5.0`, Golem **1.6.0** layout). **Status:** ✅ Complete — every `public-oplog-entry`
> case is typed, and the decoder is **generated from the WIT** (see [Generated model](#generated-model)).

## Overview

Every Golem agent is backed by an append-only *operation log* (the "oplog"): the durable record of everything the agent
did — invocations, durable host calls, spans, plugin activity, updates, snapshots, retries. This API exposes the two read
primitives Golem offers over an agent's oplog:

- [`GetOplog`](#getoplog) — read the oplog forward from a given index, in batches.
- [`SearchOplog`](#searchoplog) — full-text search across an oplog, returning matching `(oplog-index, entry)` pairs.

Both are thin wrappers over the corresponding resources (they mirror the Scala SDK's `OplogApi.GetOplog` /
`OplogApi.SearchOplog`). Each yields [`PublicOplogEntry`](#publicoplogentry) values.

Use them for auditing, debugging, activity feeds or reacting to specific past operations. They complement the durable
execution primitives on [`HostApi`](host-api.md) and [`DurabilityApi`](durability.md); see the SDK overview in
[`../../README.md`](../../README.md).

Both resources hold a host-side handle that is **not** tied to Kotlin/Wasm GC — always call `close()` when finished (a
`try`/`finally` is idiomatic).

Types live in `cloud.golem.runtime.host`. `AgentId` / `ComponentId` / `EnvironmentId` / `Uuid` / `TypedSchemaValue` are
documented in [`host-api.md`](host-api.md) and [`types.md`](types.md).

## API reference

### `GetOplog`

```kotlin
class GetOplog(agentId: AgentId, start: Long) {
    /** The next batch of entries, or null once the oplog is exhausted. Throws OplogReadException if the host refuses. */
    fun getNext(): List<PublicOplogEntry>?

    /** Releases the get-oplog handle's guest-side handle-table entry. */
    fun close()
}
```

`getNext()` returns entries in oplog order, in batches: keep calling it until it returns `null`.

### `SearchOplog`

```kotlin
class SearchOplog(agentId: AgentId, text: String) {
    /** The next batch of (oplog-index, entry) matches, or null once exhausted. Throws OplogReadException if the host refuses. */
    fun getNext(): List<Pair<Long, PublicOplogEntry>>?

    fun close()
}
```

### Errors

In 1.6.0 `get-next` returns `result<_, oplog-read-error>`. The wrappers throw `OplogReadException`; its `error` is
`OplogReadError.PermissionDenied` or `OplogReadError.InternalError(value)`.

### `PublicOplogEntry`

A sealed hierarchy with one data class (or `object` for payload-free cases) per WIT `public-oplog-entry` case, flattened so
each carries its record's fields directly (every case has a `timestamp: Timestamp`, except where the WIT has none). The cases
are, in WIT (= tag) order:

`Create`, `Start`, `End`, `Cancelled`, `AgentInvocationStarted`, `AgentInvocationFinished`, `Suspend`, `Error`, `RecoverySucceeded`, `Jump`, `Interrupted`, `Exited`, `PendingAgentInvocation`, `PendingUpdate`, `SuccessfulUpdate`, `FailedUpdate`, `GrowMemory`, `CreateResource`, `DropResource`, `Log`, `Restart`, `Resumed`, `ActivatePlugin`, `DeactivatePlugin`, `Revert`, `CancelPendingInvocation`, `BeginRemoteTransaction`, `PreCommitRemoteTransaction`, `PreRollbackRemoteTransaction`, `CommittedRemoteTransaction`, `RolledBackRemoteTransaction`, `Snapshot`, `OplogProcessorCheckpoint`, `SetRetryPolicy`, `RemoveRetryPolicy`, `CardEventQueued`, `CardInstalled`, `CardInstallFailed`, `CardRevoked`, `CardExpired`, `HostStreamFrame`, `ToolAttachmentRead`, `ToolAttachmentControl`, `StreamRegistered`, `StreamItems`, `StreamEnd`, `StreamCancel`, `StreamSession`, `CompletionDiscarded`, `CompletionDelivered`, `SnapshotConfirmed`.

Highlights:

- **Agent lifecycle:** `Create(agentId, ownerKind, agentMode, componentRevision, env, …)`, `Suspend`, `Interrupted`, `Exited`,
  `Restart`, `Resumed`, `Error(kind, error, retryFrom, retryPolicyState)`, `RecoverySucceeded`, `Jump`, `Revert`.
- **Invocations:** `AgentInvocationStarted(invocation, walletPin)`, `AgentInvocationFinished(result, methodName, consumedFuel, …)`,
  `PendingAgentInvocation`, `CancelPendingInvocation`.
- **Durable host calls:** `Start(functionName, invocationId, request, …, spanStarted, denial)`, `End(startIndex, response,
  forcedCommit, spanFinished, …)`, `Cancelled`, `CompletionDelivered`, `CompletionDiscarded`. Request and response payloads are
  generic `TypedSchemaValue`s identified by `functionName` (there are no per-interface payload variants).
- **Durable streams / tools:** `HostStreamFrame`, `StreamRegistered`, `StreamItems`, `StreamEnd`, `StreamCancel`,
  `StreamSession`, `ToolAttachmentRead`, `ToolAttachmentControl`.
- **Updates and snapshots:** `PendingUpdate`, `SuccessfulUpdate`, `FailedUpdate`, `Snapshot`, `SnapshotConfirmed`.
- **Permission cards:** `CardEventQueued`, `CardInstalled`, `CardInstallFailed`, `CardRevoked`, `CardExpired`.
- **Misc:** `Log(level, context, message, traceContext)`, `GrowMemory`, `CreateResource`, `DropResource`, plugin and
  retry-policy entries, remote-transaction markers.

Spans are no longer separate `StartSpan` / `FinishSpan` / `SetSpanAttribute` entries: they are embedded in `Start` / `End` /
`Cancelled` as `spanStarted` / `spanFinished` / `spanAttributes`.

### Generated model

`OplogEntries.kt` is **generated** by `scripts/gen-oplog.py` from `wit-native/deps/golem-1.x/golem-oplog.wit` (via
`wasm-tools component wit --json`). Layouts, tags and field offsets are computed from the WIT, never hand-derived, because this
variant changes with nearly every Golem release. After syncing the WIT (`cargo make wit`), re-run `scripts/gen-oplog.py`
(`--check` verifies the checked-in file is current). Types that already exist in the SDK (`Uuid`, `AgentId`,
`TypedSchemaValue`, `NamedRetryPolicy`, `Attribute`, …) are reused rather than regenerated; generated enum entries are
`SCREAMING_CASE`, `list<u8>` is a `ByteArray`, `u32` is `UInt` and `u64` / `s64` are `Long`.

### Changes from the pre-1.6.0 API

- Removed: `NoOp`, `BeginAtomicRegion`, `EndAtomicRegion`, `ChangePersistenceLevel`, `FilesystemStorageUsageUpdate`,
  `StartSpan`, `FinishSpan`, `SetSpanAttribute`, and `OplogPersistenceLevel` (atomic regions, persistence levels and
  filesystem quotas were removed in 1.6.0).
- `Error` has `kind` instead of `insideAtomicRegion`. `Start` gains `invocationId`, `observationalOwner`, `spanStarted` and
  `denial` (`durableFunctionType` is now the generated `WrappedFunctionType` sealed class with four objects `ReadLocal` / `WriteLocal` / `ReadRemote` / `WriteRemote`, no longer the 7-case form; it is a distinct type from the hand-written `DurableFunctionType` enum used by [`DurabilityApi`](durability.md)). `PendingUpdate` and `FailedUpdate` carry the snapshot-assisted-update fields.
  `Create` has `ownerKind`. `Snapshot` has an optional `filesystemSnapshot`.
- `CardId` is a generated record rather than a bare `Uuid`; `SnapshotData.data` is a `ByteArray`.

## Example

```kotlin
val self = HostApi.getSelfMetadata().agentId
val oplog = GetOplog(self, 0L)
try {
    while (true) {
        val batch = oplog.getNext() ?: break
        for (entry in batch) {
            when (entry) {
                is PublicOplogEntry.Start -> println("call ${entry.functionName}")
                is PublicOplogEntry.End -> println("done ${entry.startIndex}: ${entry.response?.value}")
                else -> {}
            }
        }
    }
} finally {
    oplog.close()
}

val search = SearchOplog(self, "needle")
try {
    while (true) {
        for ((index, entry) in search.getNext() ?: break) println("$index: ${entry::class.simpleName}")
    }
} finally {
    search.close()
}
```

## Verification

Decoded end to end on a real 1.6.0 executor: an agent read its own oplog (`Create`, `PendingAgentInvocation`,
`AgentInvocationStarted` with its nested typed values and invocation-context spans, `Log`, `Start` / `End` of a custom durable
call) and found a log line with `SearchOplog`. The layout engine was cross-checked against the 1.5.0 WIT, reproducing the
previously verified sizes (208 and 216 bytes).
