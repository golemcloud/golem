# GOL-476 Plan 2: Fingerprint-Scoped Derived Cache Namespaces

## Decision

Keep read-side cache warming. Physically scope every derived per-agent cache namespace by the
`AgentFingerprint` stored in the oplog `Create` entry, and treat the cached agent mode only as a
routing hint.

This plan adopts the following product decision:

- Stale Redis data left by a rare read/delete race is acceptable.
- Stale data must never be interpreted as belonging to a deleted or recreated agent incarnation.
- A large Redis TTL bounds the physical garbage in production.
- Correctness must not depend on TTL; configurations without physical expiry remain logically safe.

The proposed cache TTL is seven days. It is refreshed atomically with every cache publication. This
is long enough to preserve warming for ordinarily dormant workers while eventually removing cache
namespaces that no live incarnation refreshes.

## Core invariant

Every oplog `Create` entry has a fresh `instance_id`, exposed as `AgentFingerprint`. Recreating the
same `OwnedAgentId` creates a different fingerprint.

Derived state for two incarnations therefore lives under different physical keys:

```text
agent-status:{owned-agent-id}:{F1}
agent-status:{owned-agent-id}:{F2}

agent-stream-index:{owned-agent-id}:{F1}
agent-stream-index:{owned-agent-id}:{F2}
```

The invariant is:

> A reader may read or update only the derived namespace selected by an authoritative current
> `Create` fingerprint or by the already-authoritative fingerprint of its resident worker.

A delayed F1 writer can create or update only an F1 key. It cannot leave fields inside F2's hash,
overwrite F2's projection metadata, or make F1 data appear valid by changing one shared header.

## Why physical namespace scoping is required

Adding only a `fingerprint` field to today's shared hash is unsafe because status updates are deltas:

```text
F2 writes complete status and remembers its in-memory baseline
F1 delayed writer overwrites the shared hash
F2 writes only changed core fields plus fingerprint=F2
unchanged F1 membership/region/transfer fields remain
reader sees fingerprint F2 and incorrectly accepts mixed F1/F2 data
```

Invocation-result mappings and durable-stream projection rows have the same problem. Separate
fingerprint namespaces remove this entire class of mixed-field failures without requiring atomic
full-hash replacement or a distributed cache-publication lock.

## Cache namespace representation

Change the cache-routed per-agent namespaces to include fingerprint:

```rust
AgentStatus {
    agent_id: Arc<AgentId>,
    fingerprint: AgentFingerprint,
}

AgentStatusCheckpoint {
    agent_id: Arc<AgentId>,
    fingerprint: AgentFingerprint,
}

AgentInvocationResultIndex {
    agent_id: AgentId,
    fingerprint: AgentFingerprint,
}

AgentDurableStreamSessionIndex {
    agent_id: AgentId,
    fingerprint: AgentFingerprint,
}
```

The physical Redis hash names include the fingerprint. SQLite/PostgreSQL/memory cache backends use
the same logical namespace distinction.

`AgentStatusRecord` remains unchanged. The fingerprint is namespace identity, not agent status and
not part of the deployment-diff model.

There is no compatibility path for the old cache names. These caches are disposable, and repository
policy requires changing all in-tree producers and consumers directly.

## Fingerprint availability by data family

### Live status and clean checkpoint

`WorkerService::get` already reads the authoritative `Create` entry before reading cached status.
It uses `Create.instance_id` to select the fingerprint-scoped live status and checkpoint hashes.
This adds no oplog request to the current path.

Other status recomputation callers must receive the expected fingerprint explicitly:

- worker hydration and owner initialization use `initial_worker_metadata.fingerprint`;
- enumeration and guest metadata obtain it from the same `Create` entry used for metadata;
- promise activation and fork carry initial metadata;
- the status actor, flusher, and checkpointer receive fingerprint at construction.

Status and checkpoint writes continue using their current split-field and delta logic inside one
incarnation-specific hash.

### Invocation-result index

The resident worker already knows its fingerprint. Thread it through every invocation-index lookup,
status-driven catch-up, clear, and publication operation so they select the correct physical
namespace.

Keep the existing revert-generation and coverage metadata. Fingerprint namespace isolation is
additional to those logical versions, not a replacement.

Current invocation-index chunk publication is unconditional: it retains previously loaded coverage
in memory and calls `set_many_raw`. That is unsafe if the hash expires or Redis loses it between
loading coverage and publication. Add metadata-CAS publication to every catch-up chunk:

1. Compare the persisted metadata with the exact revert generation and coverage from which the
   chunk was computed.
2. On a match, atomically write the chunk mappings, advance metadata, and refresh expiry.
3. On a mismatch, reload persisted metadata and restart from its coverage; absent metadata means
   reconstruction from the beginning.
4. Guard revert-generation resets and field removals with the same metadata comparison, so one
   service cannot clear another publisher's newer progress.

This CAS is needed for same-incarnation cache loss and concurrent publishers; physical fingerprint
scoping independently prevents cross-incarnation mixing.

### Durable-stream session and producer index

Thread the expected fingerprint through every index operation, including:

- owner producer metadata reads and writes;
- owner topology recovery;
- raw-session reconstruction in primary and ephemeral oplogs;
- control metadata, resume offset, journal page, and recovery-catalogue reads;
- foreign attachment status and journal inspection;
- foreign producer metadata and journal-lag inspection.

Resident owner paths use their authoritative worker/producer fingerprint. Raw primary and ephemeral
oplogs retain the owner fingerprint when constructed and pass it to raw-session reconstruction.

Foreign attachment keys contain expected fingerprints, but an expected fingerprint does not prove
that incarnation still exists. Before using a foreign fingerprint-scoped index, resolve the current
authoritative `Create` identity and require it to match the expected fingerprint. Do not use the
stream projection's cached `producer_fingerprint` as proof of current existence.

Keep the stream index's current coverage CAS and all final metadata/field consistency checks. The
physical namespace prevents cross-incarnation inheritance; coverage still protects concurrent
updates within one incarnation.

### Rejected periodic snapshots

These persistent fields are already keyed by `AgentFingerprint`. No representation change is
required. They remain in persistent storage and receive no cache TTL.

## Agent-mode cache

### What is actually optimized today

Current `main` caches only `AgentMode::Durable`. Ephemeral mode is deliberately not cached because a
transient ephemeral oplog can disappear while a flat cache key remains.

Fresh ephemeral invocation admission is fast for a different reason:

1. The worker service resolves mode from agent-type/component metadata.
2. It sends a `KnownFresh` invocation.
3. Executor creation skips existing-worker and old-oplog probes.

This path must remain unchanged. A failed creation retry may intentionally fall back from
`KnownFresh` to `MayExist`; tests must preserve that behavior.

### Keep mode as a hint, not authority

Keep the mode cache as one flat, fast Redis value. It may store the fingerprint for diagnostics and
for callers that already have an expected fingerprint:

```rust
struct CachedAgentMode {
    mode: AgentMode,
    fingerprint: AgentFingerprint,
}
```

The cache hit remains one Redis read. Reading this value alone does not prove that the worker still
exists and does not authorize destructive work.

### Authoritative identity resolution

Replace correctness-sensitive uses of `get_agent_mode` with an internal identity resolver returning
the resolved `Create` entry as well as mode:

```rust
struct ResolvedAgentIdentity {
    mode: AgentMode,
    fingerprint: AgentFingerprint,
    create_entry: OplogEntry,
}

async fn resolve_agent_identity(
    &self,
    owned_agent_id: &OwnedAgentId,
) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError>;
```

Algorithm:

1. Read the cached mode hint.
2. Use it only to choose which namespace to probe first.
3. Read the authoritative `Create` entry in that namespace.
4. Any valid `Create` there is authoritative, even when its fingerprint differs from the hint. This
   handles same-mode recreation correctly.
5. If that namespace has no `Create`, probe the other mode.
6. With no hint, probe durable then ephemeral, matching the current deterministic order.
7. Refresh a durable hint with the identity that was actually found.
8. If an ephemeral identity is found, invalidate a stale durable hint rather than caching ephemeral
   mode.
9. Return `None` only when neither namespace has a `Create`; propagate malformed oplogs and storage
   failures as errors.

`WorkerService::get` consumes the returned `Create` directly instead of reading it again. Its valid
hint path therefore has the same one mode-cache read plus one `Create` read as today.

Audit and convert all callers for which wrong mode can change correctness:

- worker metadata and existence;
- revert resolution;
- gRPC public-oplog get/search;
- guest oplog get/search, enrichment, and payload resolution;
- foreign durable-stream identity, control, journal, and producer lookups;
- any path turning an empty/missing oplog result into agent absence.

An empty requested oplog page is not evidence that the hint was correct. Not-found-sensitive paths
resolve identity before the page/range read.

### Destructive operations

Destructive operations never consult the mode hint. Change `WorkerService::remove` to receive the
authoritative mode and fingerprint captured from the owning worker:

```rust
async fn remove(
    &self,
    lifecycle: &mut OplogLifecycleGuard,
    owned_agent_id: &OwnedAgentId,
    mode: AgentMode,
    fingerprint: AgentFingerprint,
) -> Result<(), WorkerExecutorError>;
```

The sole production caller already owns `initial_worker_metadata`, which contains both values.

Under the lifecycle guard, deletion distinguishes:

- **Matching `Create` fingerprint:** perform normal derived-state then oplog deletion.
- **Different current fingerprint:** never delete the current oplog or current cache namespaces;
  only old fingerprint-scoped cache cleanup and exact old recovery-member cleanup are safe.
- **No current `Create`:** finish idempotent old-fingerprint cache and recovery-member cleanup. This
  is required when a retry follows successful oplog deletion and failed final membership removal.

The archive path also calls `remove_cached_status`; extend it to identify the fingerprint whose
status/checkpoint hashes it is removing.

## `RunningWorkers`

Change persistent recovery-set members from `OwnedAgentId` to:

```rust
struct RunningWorker {
    owned_agent_id: OwnedAgentId,
    fingerprint: AgentFingerprint,
}
```

Only owner initialization and owner status transitions update this set. Generic read-side cache
warming uses the cache-only status writer and never changes assignment tracking.

Fingerprinting remains useful even after that separation:

- an old enumeration snapshot can be distinguished from a recreated incarnation;
- final-removal retries remove exactly F1 without removing F2;
- recovery can clean an absent/mismatched F1 member while preserving F2.

Recovery processing is:

1. Read `(OwnedAgentId, fingerprint)`.
2. Resolve authoritative identity.
3. If absent, remove the exact member and optionally clean that fingerprint's disposable caches.
4. If the current fingerprint differs, remove the stale member and leave the current incarnation
   untouched.
5. If it matches, acquire the worker through an **existing-only, expected-fingerprint** path.
6. Revalidate the fingerprint at the guarded acquisition/admission point before activation.

The final revalidation is mandatory. Current recovery eventually calls `get_or_create_running`; a
delete between scan-time validation and activation must not create a new incarnation or activate a
replacement under the stale recovery decision.

## Cache publication after TTL expiry or Redis loss

Physical fingerprint scoping prevents F1/F2 mixing, but a resident F2 status writer can still retain
an in-memory delta baseline after Redis loses the F2 hash. Publishing only a delta into an empty hash
would create an incomplete F2 status.

Strengthen split-status/checkpoint publication:

1. A delta publication compares the persisted `core` with the exact core represented by the
   writer's in-memory baseline.
2. If it matches, atomically apply changed sets/deletions and the new core.
3. If it is absent or differs, read all existing fields and compute complete sets plus stale-field
   deletions for that observed snapshot.
4. Compare the observed `core`, atomically apply those full sets/deletions plus expiry, and retry the
   read/recompute cycle on a mismatch.
5. Extend the cache storage CAS operation to support guarded field removals as well as sets.

Do not retain the current unguarded `previous_status = None` path: it lists fields, deletes some, and
then writes fields in separate operations. A concurrent newer publication can otherwise leave
auxiliary fields from one status beside another status's core. Foreground cold readers,
same-incarnation fallback, live status, and clean checkpoints all use the guarded reconciliation.

The stream index already uses metadata/coverage CAS. The invocation index gains equivalent guarded
publication as described above. For both, an absent metadata row starts reconstruction from no
coverage and cannot inherit another fingerprint's namespace.

## TTL implementation

TTL is a physical-retention mechanism, not part of the logical proof.

Add explicit cache-key/hash publication operations that establish or refresh expiration atomically:

```rust
set_with_expiry(..., ttl)
set_many_with_expiry(..., ttl)
compare_and_set_many_with_deletes_and_expiry(..., ttl)
```

Requirements:

- Redis applies the write/CAS/deletions and `EXPIRE` atomically, using the existing transaction/Lua
  layer as appropriate.
- Flat mode-key expiry targets that one physical key; per-agent status/index expiry targets that one
  physical hash.
- Namespace routing permits expiry only for cache-routed namespaces and rejects accidental expiry of
  persistent namespaces such as `RunningWorkers`.
- Retrying, fault-injecting, labelled, memory, SQLite, PostgreSQL, and multi-SQLite adapters implement
  or explicitly propagate the new operation.
- Cache backends without physical expiry may treat expiration as a documented best-effort no-op;
  fingerprint scoping still guarantees logical correctness, but stale bytes may remain indefinitely.
- No separate best-effort `EXPIRE` after a successful write: a crash in that gap could leave the key
  forever, contradicting the accepted TTL mitigation.

The compare-and-set operation returns mismatch without mutation. Status/checkpoint callers then
reread all fields and recompute full reconciliation; invocation/stream callers reload metadata and
recompute catch-up from persisted coverage.

No expiry operation occurs on a cache-hit read. Expiry accompanies existing cache publications,
including foreground reconstruction and projection catch-up.

## Deletion protocol

Retain the worker-owned deletion stages, stream deletion prerequisites, status/checkpoint flusher
barriers, and oplog owner joins. Then:

1. Capture authoritative mode, fingerprint, and shard from the owning worker before storage cleanup.
2. Under the lifecycle guard, classify current `Create` as matching, different, or absent.
3. Delete the deleting fingerprint's disposable namespaces:
   - live status;
   - clean checkpoint;
   - invocation-result index;
   - durable-stream index;
   - mode hint only if its cached fingerprint still matches the deleting fingerprint.
4. Delete persistent per-fingerprint derived state such as rejected periodic snapshots.
5. If any derived-state operation fails, return the error while the matching oplog and exact
   `RunningWorkers` member still exist. Explicit retry repeats idempotent cleanup.
6. For a matching `Create`, delete the oplog using its authoritative mode. For different/absent
   `Create`, never perform unconditional oplog deletion.
7. Remove the exact `(OwnedAgentId, fingerprint)` recovery member last.

Normal ordering is therefore:

```text
F1 caches -> F1 persistent derived state -> F1 oplog -> RunningWorkers(F1)
```

A cache delete/read race may recreate only F1 cache keys, which no F2 reader opens and TTL later
expires. No read-side warming can recreate persistent recovery membership.

## Concurrent read/delete/recreate examples

### Deletion without recreation

```text
reader                              deleter
------                              -------
resolve Create(F1)
                                    delete F1 cache hashes
                                    delete oplog F1
                                    remove RunningWorkers(F1)
publish reconstructed F1 cache
```

The F1 cache can remain physically until TTL. A later identity resolution finds no `Create`, so no
reader selects the F1 namespace and the worker remains logically absent.

### Same-ID recreation

```text
old reader writes:  agent-status:{id}:{F1}
new worker reads:   agent-status:{id}:{F2}
```

The keys are disjoint. A stale F1 mode hint merely chooses a probe order: a valid same-mode F2
`Create` is accepted as authoritative, while an opposite-mode recreation is found by fallback.

### Redis loss during a resident worker's lifetime

The next status delta CAS observes missing persisted core and falls back to guarded complete
reconciliation. Invocation catch-up observes missing metadata and restarts from the beginning. Neither
can create a partial hash from a remembered in-memory baseline.

## Performance impact

### Warm paths

- Status/checkpoint still use one hash read; the fingerprint changes the physical key, not request
  count.
- Invocation and stream indexes retain their existing metadata reads; invocation publication gains
  CAS while stream publication retains its existing CAS.
- A durable mode-hint hit remains one Redis read.
- `resolve_agent_identity` uses one hinted `Create` read in the normal case and returns that entry to
  its caller, avoiding a duplicate read.
- Fresh ephemeral `KnownFresh` admission remains unchanged and does not call `get_agent_mode`.
- No distributed lock or persistent deletion-intent read is added to a hot path.

### Cold and exceptional paths

- Missing/stale mode hints may require probing both namespaces.
- Redis loss causes one failed status delta CAS followed by complete status reconciliation, or one
  failed invocation publication CAS followed by catch-up restart from persisted coverage.
- A seven-day inactive namespace expiry causes the next read to reconstruct and warm once, preserving
  amortization for subsequent reads.
- Foreign stream validation performs authoritative identity resolution rather than trusting cached
  projection identity. Measure this path because stream-segment authorization is data-plane work.

### Storage and write amplification

- Physical key names gain one UUID.
- Expiry is combined with existing publications rather than sent as a separate request.
- Fingerprint-scoped stale hashes may coexist until TTL, temporarily increasing Redis usage after
  delete/recreate races.
- Background status publication remains coalesced at the existing interval; there is no new
  per-invocation write.

## Metrics

Add low-cardinality counters for:

- mode-hint hit, no-hint, alternate-mode fallback, and stale-hint replacement;
- stale `RunningWorkers` member removed because identity is absent or fingerprint differs;
- status delta-CAS match versus complete-reconciliation fallback;
- foreign stream expected/current fingerprint mismatch;
- cache-expiry publication failure by cache kind.

Do not use agent IDs or fingerprints as labels.

## Tests

### Namespace isolation

1. F1 and F2 status/checkpoint hashes can coexist and never mix fields.
2. A delayed F1 status publication after complete F2 publication and a later F2 delta cannot affect
   any F2 field.
3. F1 invocation mappings with the same revert generation as F2 are invisible to F2.
4. F1 stream controls, producer rows, journal pages, resume offsets, and recovery catalogue are
   invisible to F2 and cannot seed F2 reconstruction.
5. Raw primary and ephemeral session reconstruction select their owning fingerprint namespace.

### Mode and identity

1. A valid durable hint takes one hinted `Create` read and performs no alternate probe.
2. Durable hint F1 plus durable `Create(F2)` returns F2, not absence.
3. Durable hint F1 plus ephemeral `Create(F2)` falls back to ephemeral and invalidates the durable
   hint.
4. A stale hint with neither oplog returns absent after both probes.
5. Malformed oplog and storage errors remain errors rather than cache misses.
6. Revert, public/guest oplog lookup and enrichment, and foreign stream reads all use authoritative
   identity rather than mode alone.
7. Normal fresh ephemeral `KnownFresh` creation performs no mode/oplog existence probe; a retry after
   partially successful creation may use `MayExist` as today.

### Cache loss and TTL

1. Remove the status hash while retaining a flusher's baseline; its next delta restores a complete
   status, including unchanged membership, regions, updates, and transfers.
2. Repeat for checkpoint publication.
3. Pause a cold/fallback status publisher after its field snapshot; let another service publish a
   newer status containing an additional transfer field; resume and verify all resulting fields
   consistently represent one published status. Repeat for the clean checkpoint.
4. Pause invocation catch-up after loading nonzero coverage; delete or expire its hash; resume and
   verify an invocation below the old coverage remains discoverable rather than becoming a
   definitive miss.
5. Concurrently reset and publish an invocation index through independent services; verify guarded
   reset cannot remove newer progress.
6. Verify Redis publication and expiry are atomic from the storage API's observable behavior.
7. Verify cache-hit reads do not refresh TTL or add storage calls.
8. Verify persistent namespaces reject expiry.

### Deletion failure and interleavings

1. Pause a foreign service immediately before publishing reconstructed F1 status; delete F1; resume
   publication; assert identity resolution remains absent despite the physical F1 hash.
2. Repeat with F2 recreation and assert every F2 cache/index read remains isolated.
3. Inject failure at every derived-delete boundary and assert matching oplog plus
   `RunningWorkers(F1)` remain for explicit retry.
4. Fail final membership removal after oplog deletion; retry on the same owner with absent `Create`
   and assert cleanup succeeds.
5. Repeat final-removal failure across restart and assert recovery removes F1.
6. Pause recovery after scan-time F1 validation; delete or recreate; resume and assert existing-only
   expected-fingerprint acquisition neither creates a worker nor activates F2 under F1's decision.
7. Release a delayed F1 operation after F2 joins recovery membership; assert exact F1 cleanup keeps
   F2.

Use independently constructed services sharing storage for publication/delete races so tests do not
pass solely because of the process-local lifecycle gate.

### Performance/counting tests

1. Warm status/checkpoint lookup adds no storage request relative to current hash lookup.
2. Warm invocation/stream index lookup adds no request for fingerprint validation.
3. Normal durable identity resolution probes only the hinted namespace.
4. A cold cache reconstructs and publishes once; the next read uses the fingerprint-specific cache.
5. Expiry accompanies publication without a second cache request.
6. Fresh ephemeral invocation preserves the current no-probe path.

## Explicit limitations

- Rare stale F1 cache namespaces can exist physically until TTL.
- Backends that do not implement physical expiry can retain those disposable namespaces
  indefinitely, though no current incarnation can read them.
- The mode cache is only a routing hint. Every destructive and not-found-sensitive path must resolve
  authoritative identity.
- Foreign expected fingerprints are claims, not existence evidence; foreign index reads validate
  current `Create` identity first.
- This plan does not fence an old shard owner that continues destructive oplog or persistent-index
  work after forced takeover. That remains part of shard epoch/oplog deletion fencing.

## Implementation sequence

1. Add fingerprints to cache namespace variants and update all storage encodings.
2. Thread fingerprint through status/checkpoint readers, writers, actors, flushers, and checkpointers.
3. Thread fingerprint through invocation-result index lookup, catch-up, clear, and metadata paths.
4. Thread fingerprint through every stream-index owner, raw-session, final-field, and foreign path.
5. Introduce `CachedAgentMode` and authoritative `resolve_agent_identity`; audit every mode caller.
6. Change `RunningWorkers` members and add existing-only expected-fingerprint recovery acquisition.
7. Separate generic cache warming from owner-only assignment tracking.
8. Make all status/checkpoint publication use guarded sets/deletions, including cold and fallback
   reconciliation after missing/replaced physical baselines.
9. Add metadata-CAS to every invocation catch-up chunk and guard revert-generation resets.
10. Pass authoritative mode/fingerprint into deletion and implement matching/different/absent retry
   branches.
11. Reorder normal deletion to fingerprint-scoped derived state, oplog, then exact recovery member.
12. Add atomic seven-day cache publication expiry across the key-value storage adapters.
13. Add correctness, interleaving, cache-loss, TTL, recovery, and performance-counting tests.

Land cache representation changes and all in-tree consumers together. No compatibility aliases,
fallback parsing, or migration path are required for disposable old-format cache entries.
