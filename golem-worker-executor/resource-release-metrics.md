# Resource release metrics

These Prometheus metrics describe process-local owner facts. They do not drive interruption,
cleanup, retries, billing, replay, or recovery. Worker/WorkerInstance and retained StopProgress
remain the lifecycle authority. `/metrics` exports the existing Prometheus registry.

## Histograms

Every histogram below uses seconds and these finite bucket boundaries:

`0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300`

Text export also includes `+Inf`, `_sum`, and `_count`. Shared default buckets are unchanged.
Thirty seconds is a bucket boundary, not a release deadline.

### `golem_agent_resource_release_seconds{origin,stage,cause,outcome}`

`origin=accepted_stop` starts at the first retained stop leader, before its driver runs.
The cause remains `pending` for unfinished collection until the existing election freezes it.
Completed durations use only that frozen cause. Followers reuse the interval and original start.

`origin=unload` starts when ownership transfers to module-owned unload, unless an accepted stop
already owns the disposal. Ordinary unload works when all billing meters are disabled.

If unload starts first and a stop is accepted while its cleanup is pending, both intervals are
retained. The unload interval keeps its original start and cause. One accepted-stop interval starts
at the later acceptance. They share scalar physical facts, not a second deletion or release.
Stages completed before the later acceptance are inapplicable to that interval, not zero-duration
releases. A later stop-driver join belongs to the accepted interval, not to the preceding unload.

| Stage | End fact |
| --- | --- |
| `primary_execution_quiesced` | Primary execution has left the interval committed to teardown. A filesystem-limit branch that resumes the same runtime does not end it. Destruction also proves quiescence on exceptional paths. |
| `primary_store_drop` | Destruction of the actual primary Store has returned, including failed or cancelled core initialization and memory reconciliation. |
| `memory_grant_release` | The captured reservation-bearing MemoryGrant has returned its reservation and is being destroyed. Removing its registration is not release. A retained grant stays pending; merging transfers its receipts to the surviving grant. Inert grants are absent. |
| `filesystem_drained` | Admitted calls and native nodes have drained under the sealed generation. |
| `window_prepared` | Existing monitor/accepted-driver joins and final allocation freeze have returned. This does not close billing. |
| `filesystem_deleted` | The owning adapter has verified deletion, including explicit repair. |
| `permit_released` | The real scheduler or bypass permit's Drop has returned its slot/raw semaphore permit. Billing closes first under the existing owner, including when the window holds the permit. |
| `permit_wait_joined` | The exact accepted startup task has been aborted and joined, and that attempt had no tracked primary Store, filesystem, or concurrent permit. |
| `end_to_end` | The last applicable owner receipt, including retained cleanup, tracked reservation return, and accepted-driver joins. Startup attachment remains open until the existing owner finishes attaching obligations. |

`outcome=released` means the physical endpoint was observed without a retained cleanup failure at
that observation. `released_with_failure` means physical completion with retained failed health;
it is not successful cleanup. Earlier observations and lifecycle errors are immutable. A later
failure does not rewrite an already emitted stage, and a repair does not erase failure history.
Unverifiable or permanently pending work has no completed duration.

### `golem_agent_stop_host_operation_exit_seconds{operation,outcome}`

The only operation is `p3_tcp_receive`. It measures acceptance-to-exit for a receive driver that
was already entered at acceptance. Its endpoint is the actual spawned driver's return, trap, or
destruction, including the durable receive path, with outcomes `returned`, `trapped`, or `dropped`.

The initial synchronous return of stream/future handles, native select completion, and durable End
are not this endpoint. A driver entered after acceptance is not attributed to that earlier stop.
This is not universal host-function-exit coverage and none of these outcomes proves resource release.
Entered primary DB/KV work still completes backend work and required durable finalization.

### `golem_agent_filesystem_lifecycle_seconds{operation,outcome}`

`operation=delete` starts when the owning filesystem generation accepts logical deletion,
including its drain. Verified deletion ends it. The original start survives failed attempts and
explicit repair. Outcomes are `success` and `success_after_failure`.

This previously unwired family now records real deletion. Unresolved failures have no completed
sample. Already-deleted joins and stale retries do not record another deletion. Its local interval
differs from accepted-stop-to-filesystem-deleted and is observed only once per owning generation.

## Failures and unfinished work

`golem_agent_resource_cleanup_failures_total{stage,reason}` counts a newly installed bounded failure
fact once per logical obligation/stage/reason. Joining an old error, another same-reason repair
failure, or a second origin does not duplicate it. The reasons are:

`deadline`, `filesystem_observation`, `filesystem_delete`, `exclusive_ownership`, `meter_fault`,
`observer_lost`, `monitor_join`, `stop_driver`, `panic`, `other`.

Classification occurs at typed owner sites before error formatting. Wrapping an immutable retained
cleanup error does not classify or count it again. A new window-opening failure is classified where
opening returns: filesystem observation errors remain `filesystem_observation`, already-open and
stopped-memory-meter errors are `meter_fault`, and opening cancellation is `other`. No errors, paths,
URLs, account IDs, agent IDs, or invocation IDs appear in labels.

`golem_agent_resource_release_unfinished{origin,stage,cause,health}` counts outstanding applicable
receipts. `stage=end_to_end` counts intervals instead. Do not sum stage series as a distinct-worker
count. `health` is `pending` or `failed`; end-to-end retains any owner failure, while stage health
reflects a failure assigned to that stage. An early deadline or a dropped observer never completes
a physical receipt.

`golem_agent_resource_release_oldest_unfinished_age_seconds{origin,stage,cause,health}` reports the
oldest matching unfinished interval's monotonic age. It advances during registry collection without
a polling task, timer, subscription, watchdog, or resource-owner lookup. Seen partitions are emitted
as zero after completion or reclassification.

Origins are `accepted_stop`, `unload`, and `filesystem_delete`. Standalone filesystem deletion uses
`stage=filesystem_deleted,cause=deleting`; deletion attached to worker unload has its unfinished
receipt on the worker interval instead. `host_operation_exit` is an additional unfinished stage for
tracked receive drivers.

Causes are the 14 UnloadReason names in snake case:

`deleting`, `explicit_stop`, `failure`, `filesystem_limit`, `filesystem_pressure`, `idle`,
`interrupt`, `memory_limit`, `memory_pressure`, `out_of_memory`, `panic`, `restart`, `shard_lost`,
`suspend`, plus `jump`, `monthly_compute`, `monthly_memory`, `monthly_durable_storage`,
`monthly_ephemeral_storage`, and collection-only `pending`.

## Identity, lifetime, and limits

Scalar attachment identities follow the existing startup/resident transitions under their existing
locks. The retained publication captures its scope before spawning its driver. Physical grant,
Store, permit, window, and filesystem owners carry metadata-only receipts for that exact scope.
The primary InstanceHost captures its scope before Store creation. The Store constructor attaches
receipts before setup or core initialization can fail or suspend. The existing fuel guard, hosted
instance, and running runtime transfer that same wrapper without completing it. Entity Stores do
not attach primary-Store receipts.
The same reservation can legitimately remain captured by successive resident intervals until its
real final owner releases it. No metric handle retains a Worker, Store, permit, filesystem, future,
or owner-capturing callback. The collector takes only its own ledger lock.

Physical endpoint timestamps are captured before acquiring the telemetry ledger lock. A scalar
ordering stamp disambiguates equal clock values and late receipt delivery. Internal reporting barriers
delay publication until cleanup health is known but do not move the last physical endpoint timestamp.
Completed receipt
metadata is pruned after all applicable observations; starts whose timestamps precede ledger
registration temporarily protect the facts they may need. Completed and unused scopes are reclaimed
when their last metadata handle disappears. Missing proof remains visible rather than becoming a
successful Drop observation.

Counters and histograms are cumulative until process/registry reset. Unfinished records are local
telemetry, not durable cleanup intent or cross-process recovery. Restart resets them. There is no
global quota-exhaustion timestamp or detection-delay measurement here. Store destruction does not
prove OS page reclamation or release of shared compiled-component charges. Provider waits,
filesystem drain, cleanup, and retained reservations can remain unbounded. Alert thresholds are
outside this catalogue.
