# Exact-path filesystem inspection

Read this when changing external file reads, their queueing, activation, filesystem isolation,
or worker lifecycle interactions. This is an executor observation protocol, not an HTTP handler
or a durable stream. Initialization/reconstruction can execute guest code; inspection itself
does not invoke an exported method or append an invocation to the oplog.

The CLI's existing file-contents command already reaches this executor operation through REST.
The metadata-first stream replaces that operation's full-buffer implementation; it is not a
second filesystem API. HTTP filesystem bindings reuse it, while static initial-file serving
reads blob storage without activating an agent. Directory listing remains a separate operation.

## Ownership and wire boundary

- `golem-common/src/model/filesystem/mod.rs` owns raw read-path validation, `FileByteSelection`, metadata,
  status and typed errors, including protobuf conversions. The Rust executor API receives the
  existing `CanonicalFilePath` from `base_model/path.rs`; gRPC carries it as the string `file_path`. Root/suffix composition,
  decoded-segment checks and trailing-directory intent belong to the HTTP layer. At the executor
  boundary the raw path is revalidated as a canonical absolute file path without URI decoding.
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
  The existing REST inspection consumer retains its Read authorization and requests Full for the
  exact canonical path it derived.

## Scheduling through production

1. Shared `ActiveAgents` activation survives an individual waiter's cancellation. Existing
   initialization, replay, failure, update and ephemeral lifecycle rules apply. There is no
   filesystem-read-specific admission limit, semaphore, timeout or error.
2. Reads and listings enter the existing `QueuedWorkerInvocation` deque. The constructor must
   complete first: creation reserves its idempotency key before ordinary invocation admission,
   so a pending constructor is recognized at the head of the durable queue. After initialization,
   the loop processes resident commands at invocation boundaries before pending updates or methods. They observe
   the agent's current filesystem, regardless of how many invocations were queued first; there is
   no cross-queue arrival ordering or oplog-position marker. An active invocation, including its
   streaming production, must finish before inspection runs. Cancelled resident work is pruned at
   enqueue, selection, idle/eviction, restart and cleanup boundaries; durable invocations are never
   pruned on caller disconnect.
3. `Invocation::read_file` acquires the exclusive `OwnerLane`, then
   calls `services/agent_filesystem/lifecycle/inspection/mod.rs::open_file_for_inspection`. It walks
   descriptor-relative components with no-follow semantics through the final target. Any symlink
   is forbidden. Only a regular file can be opened; directories, special files and permission
   failures are forbidden.
   Missing targets are absent. There is no implicit index or directory listing in this protocol.
4. Metadata comes from that open descriptor, and Full/Bounded/OpenEnded/Suffix selection is
   resolved against its length. `inspection_stream/mod.rs::produce_file_read` runs in the invocation
   loop, not a detached task. It holds the resident Store, generation, descriptor and owner lane
   through production. A capacity-one channel carries at most 64 KiB per chunk;
   only the bounded chunk length is converted to usize, never whole-file length.
5. Once all selected bytes have been copied into the bounded response, production is complete and
   filesystem/execution ownership is released without waiting for consumer-observed EOF. No mutable
   filesystem work overlaps producer reads. Failures are published before channel close, so a
   terminal error never appears as EOF. Premature EOF aborts instead of silently truncating a head.

## Policy, lifecycle and generation are different

The exact path is immutable for the request. A caller may derive it from a selected deployment's
exposure policy, but inspection does not force the agent back to that deployment's revision.
Normal activation selects the current lifecycle revision and reconstructs its filesystem. An
already admitted read finishes from its pinned generation; later reads see the new generation
at the same exact path. A removed file is a miss, never a reason to select another path.

Updates, revert, invocation writes/rename/delete and entity-body filesystem operations serialize
against the active read. Unload cannot cause a response to switch descriptors or generations:
the response either finishes coherently or aborts. Later reads on a retained Worker reconstruct
after unload rather than assuming that finding a cached Worker means a live instance exists.

Pending inspections are disposable observations, not durable invocations. Ordinary Suspend
preserves the queued request within this process. It waits
for normal resume; do not add an inspection-specific restart loop that defeats timed waits,
fuel admission or quota suspension. Terminal lifecycle stops fail pending inspection. Executor
loss aborts the external stream; there is no durable read reattachment contract.

## Tests that distinguish the contracts

- `tests/filesystem_inspection.rs` loads real shared corpus vectors from
  `golem-service-base/tests/fixtures/http-handlers/corpus.json`, with IDs in assertion failures.
  It exercises actual guest initialization, concurrent first reads, replay after unload,
  initializer failure, prior writes, EOF/drop/update ordering and the same exact path across update.
- `services/agent_filesystem/lifecycle/{inspection,inspection_stream}/tests.rs` tests descriptor
  safety, range boundaries, bounded chunks, early EOF, backpressure without expiry and consumer cleanup.
- `worker/mod.rs` tests transient cancellation pruning and lifecycle failure delivery.
- `grpc_read_waits_for_blocking_invocation_and_completes` proves a real gRPC read waits behind
  guest work and returns its final bytes. The Suspend runtime test proves a queued read survives
  actual unload and completes after normal resume with the active invocation's final bytes,
  before a write that was already queued when the read arrived.

Use the testing skill for fixture builds and test-r filters. Bun corpus checks only verify fixture
integrity; they are not executor conformance tests. HTTP Range syntax, status codes, validators,
route matching and deployment descriptors remain outside this protocol.
