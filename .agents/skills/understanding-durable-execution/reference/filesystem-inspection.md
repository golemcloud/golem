# Root-bounded filesystem inspection

Read this when changing external file reads, their queueing, activation, filesystem isolation,
or worker lifecycle interactions. This is an executor observation protocol, not an HTTP handler
or a durable stream. Initialization/reconstruction can execute guest code; inspection itself
does not invoke an exported method or append an invocation to the oplog.

## Ownership and wire boundary

- `golem-common/src/model/filesystem.rs` owns `FileReadTarget`, `FileByteSelection`, metadata,
  status and typed errors, including protobuf conversions. `Exact { file_path }` is a canonical
  absolute file path; `WithinRoot { root, suffix, directory_request }` retains a canonical root,
  checked decoded segments and trailing-directory intent. The executor validates again at its
  filesystem boundary. It does not decode percent escapes or normalize unsafe paths.
- `golem-api-grpc/proto/golem/worker/filesystem.proto` and
  `workerexecutor/v1/worker_executor.proto` carry this contract. A stream begins with one metadata
  head or a failure, then bounded byte chunks or a terminal failure. Metadata distinguishes
  regular file, absent and forbidden. Length, offset and selected length are u64; unsatisfiable
  is explicit. Metadata-only ignores byte ranges and emits no body. An optional timestamp is
  descriptive metadata, not a live-file HTTP validator.
- `golem-service-base::model::FileReadResponse` pairs the head with a typed byte stream.
  `golem-worker-service/src/service/worker/client.rs` validates the first frame inside the routing retry
  loop, then owns the tonic stream directly. It checks framing, chunk size and selected byte
  count. It neither retries after the head nor drains a stream into an intermediate pump.
  The existing REST inspection consumer retains its Read authorization and requests Exact/Full.

## Admission through consumer termination

1. `grpc/mod.rs::get_file_contents` captures executor arrival before metadata lookup, computes
   one absolute deadline and reserves capacity before activation. `FileReadAdmission` tracks
   active plus queued requests, including absent agents. Defaults are one active read per
   agent, 16 additional queued per agent, 128 outstanding per executor and 60 seconds total.
   `GolemConfig.file_read` configures queue/total capacity and timeout; one active turn is fixed.
   Full capacity fails immediately, without spawning a waiter. No file-size limit is implied.
2. Shared `ActiveAgents` activation survives an individual waiter's cancellation. Abandoning
   inspection removes its reservation, not initialization or its durable effects. Existing
   failed/interrupted/ephemeral lifecycle rules still apply.
3. `Worker::read_file` records `InspectionOrder` while holding the instance mutex, the same
   boundary as invocation acceptance. Its durable-invocation cutoff prevents inspection from
   passing earlier work even when that work is not resident in the local queue. Pending updates
   block inspection. `QueuedInspectionGuard` removes a cancelled read synchronously rather
   than waiting for the invocation loop to reach it.
4. `Invocation::read_file` acquires the per-agent read turn and the exclusive `OwnerLane`, then
   calls `services/agent_filesystem/lifecycle/inspection.rs::open_file_for_inspection`. It walks
   descriptor-relative components with no-follow semantics, including the selected root and
   final target. Any symlink is forbidden, even one pointing inside the root. Only a regular
   file can be opened; directories, special files and permission failures are forbidden.
   Missing targets are absent. There is no implicit index or directory listing in this protocol.
5. Metadata comes from that open descriptor, and Full/Bounded/OpenEnded/Suffix selection is
   resolved against its length. `inspection_stream.rs::produce_file_read` runs in the invocation
   loop, not a detached task. It holds the resident Store, generation, descriptor, owner lane
   and admission through the response. A capacity-one channel carries at most 64 KiB per chunk;
   only the bounded chunk length is converted to usize, never whole-file length.
6. Enqueueing the final chunk is **not** completion. The producer waits for consumer EOF, drop,
   deadline or failure. A metadata-only, empty or unsatisfiable response releases after its head.
   Drop and timeout reclaim every read guard and reservation, including when a body is not
   polled. Premature EOF and post-head errors abort instead of silently truncating a response.

## Policy, lifecycle and generation are different

The target value is immutable for the request. A caller may derive it from a selected deployment's
exposure policy, but inspection does not force the agent back to that deployment's revision.
Normal activation selects the current lifecycle revision and reconstructs its filesystem. An
already admitted read finishes from its pinned generation; later reads see the new generation
under the same selected target. A removed file is a miss, never a reason to choose a wider root.

Updates, revert, invocation writes/rename/delete and entity-body filesystem operations serialize
against the active read. Unload cannot cause a response to switch descriptors or generations:
the response either finishes coherently or aborts. Later reads on a retained Worker reconstruct
after unload rather than assuming that finding a cached Worker means a live instance exists.

Pending inspections are disposable observations, not durable invocations. Ordinary Suspend
preserves the queued request within this process, with its original arrival deadline. It waits
for normal resume; do not add an inspection-specific restart loop that defeats timed waits,
fuel admission or quota suspension. Terminal lifecycle stops fail pending inspection. Executor
loss aborts the external stream; there is no durable read reattachment contract.

## Tests that distinguish the contracts

- `tests/filesystem_inspection.rs` loads real shared corpus vectors from
  `golem-service-base/tests/fixtures/http-handlers/corpus.json`, with IDs in assertion failures.
  It exercises actual guest initialization, concurrent first reads, replay after unload,
  initializer failure, prior writes, EOF/drop/update ordering and unchanged roots across update.
- `services/agent_filesystem/lifecycle/{inspection,inspection_stream}.rs` tests descriptor
  safety, range boundaries, bounded chunks, early EOF, unpolled deadlines and consumer cleanup.
- `services/file_read_admission.rs` tests full queues and deadline accounting before activation;
  `worker/inspection_queue.rs` tests durable cutoff ordering, pending updates and cancellation.
- `configured_read_deadline_reaches_grpc_admission` proves nondefault configuration reaches real
  gRPC requests blocked behind guest work. The Suspend runtime test proves a queued read survives
  actual unload and completes after normal resume with the earlier invocation's final bytes.

Use the testing skill for fixture builds and test-r filters. Bun corpus checks only verify fixture
integrity; they are not executor conformance tests. HTTP Range syntax, status codes, validators,
route matching and deployment descriptors remain outside this protocol.
