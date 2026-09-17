# GOL-522: roll stable indexed scans out to agent enumeration

## Goal

Move the remaining production callers of `IndexedStorage::scan` to
`scan_stable`, then remove the positional/OFFSET scan API and all five backend
implementations.

The behavior visible to callers remains paginated agent enumeration across the
configured oplog layers and requested agent modes. The pagination token changes
from exposed numeric implementation state to an opaque string. This is a direct
contract replacement: repository policy requires updating every in-tree
producer and consumer and explicitly forbids compatibility parsing for the old
`<layer>/<number>` token.

## Why this change is needed

Postgres and SQLite implement `scan` using `SELECT DISTINCT ... LIMIT ...
OFFSET ...`. A complete walk is quadratic in the number of distinct keys and is
especially expensive for the primary oplog namespace because each agent key can
have thousands of oplog-entry rows. A numeric key budget limits the result page,
not the number of index tuples read to reach its offset.

`scan_stable` instead uses backend-owned continuation state:

- Postgres, SQLite, and in-memory storage seek after the last key returned.
- Multi-SQLite resumes after the last database file completed.
- Redis resumes with its native `SCAN` cursor.

The existing stable-scan contract is sufficient for listing: a key present for
the whole walk is returned at least once; keys inserted or deleted during a walk
have no snapshot guarantee. Redis may return a key more than once, as allowed by
`SCAN`.

## Current cursor state machine

Agent enumeration can traverse two independent dimensions:

1. The oplog layer: primary first, followed by each archive layer.
2. The agent mode within an indexed layer: durable first and then ephemeral
   when the filter requires both modes; otherwise only the explicitly selected
   mode.

The current `ScanCursor { layer, cursor: u64 }` exposes the layer and packs the
mode into the high bit of `cursor`. The remaining bits are the storage cursor.
Zero means both "start this storage scan" and, when returned, "this scan is
finished". `MultiLayerOplogService` interprets that terminal value and advances
to the next layer.

This representation cannot carry the string marker produced by ordered and
multi-SQLite `scan_stable` implementations. It also exposes storage details and
would place either an indexed key or a multi-SQLite filename directly in the
public representation if extended naively. The encoding below makes the token
opaque as an API contract, but base64 is not encryption and provides no
confidentiality.

## Settled public cursor contract

Replace `ScanCursor` with an opaque string newtype in
`golem-common/src/base_model/mod.rs`:

```rust
pub struct ScanCursor(String);
```

It must derive the same common serialization traits as the current type. Use
transparent string serialization for serde and `desert_rust::BinaryCodec`,
following the existing `TransactionId` string-newtype precedent. Do not derive
`poem_openapi::NewType`: with the pinned poem-openapi version that would inline
the wrapped string schema, remove the named `ScanCursor` component, and bypass
the existing shared-type mapping in `golem-client/build.rs`. Instead, implement
the small poem-openapi traits manually, following the named-schema registration
pattern used by `NormalizedJsonValue`, so OpenAPI retains a named `ScanCursor`
component whose schema is `type: string` and whose JSON value is a string.

Provide only the operations needed by transport code:

- `ScanCursor::new(String)` or `From<String>`
- `as_str()`
- `into_inner()`
- `Display`
- `FromStr`, which wraps a transport token without attempting executor-private
  semantic decoding
- `Default`, represented by the empty string
- `is_finished()` and `into_option()`, for which the empty string is terminal

The empty default remains the internal first-page sentinel. It is never emitted
as a response cursor because `into_option()` converts it to `None`. Every active
continuation, including the beginning of a new mode or layer whose backend
resume is `None`, has a non-empty encoded token. This preserves the useful
existing call shape while removing the numeric assumptions.

The token is an opaque continuation hint, not an authorization credential. It
does not need signing. Authorization and component/environment selection remain
properties of the listing request. A token is not guaranteed to survive a
deployment that changes the indexed-storage backend.

## Executor-owned token encoding

Define the decoded state privately in
`golem-worker-executor/src/services/oplog/mod.rs`:

```rust
struct OplogScanState {
    layer: usize,
    mode: AgentMode,
    resume: Option<ScanResume>,
}
```

Encode non-terminal state as:

```text
gsc1_<URL_SAFE_NO_PAD(JSON(OplogScanState))>
```

Give `ScanResume` an explicit stable serde representation rather than relying on
the default Rust enum encoding:

```json
{"layer":0,"mode":"Durable","resume":{"type":"marker","value":"<opaque>"}}
{"layer":0,"mode":"Ephemeral","resume":{"type":"cursor","value":123}}
{"layer":1,"mode":"Durable","resume":null}
```

The exact JSON field names, resume tags, and mode strings above are part of the
`gsc1_` token format. Use the existing `AgentMode` serde spelling rather than
changing shared mode serialization. Keep the codec private to the executor. Add
the existing workspace `base64` dependency to `golem-worker-executor`; `serde`
and `serde_json` are already available.

Decoding must reject, as `WorkerExecutorError::invalid_request`:

- a missing or unknown prefix/version
- invalid base64 or JSON
- unknown mode or resume variant
- an encoded layer outside the configured multilayer stack
- a mode incompatible with an explicitly single-mode request
- a resume variant rejected by the active backend
- a marker with contents that the active backend cannot bind or compare,
  including an embedded NUL

Do not let bad cursor input enter the ordinary oplog `retry_storage_op` path:
that helper intentionally panics on permanent indexed-storage errors. Introduce
a typed invalid-resume/cursor error and validate both resume variant and contents
in each backend before storage I/O. A scan-specific retry wrapper must map that
typed error to `invalid_request`, retry only `Transient`, and retain the
established panic policy for genuine permanent storage failures. Do not infer
invalid input by matching database error messages. Blob archive scans have no
backend continuation and must reject every `Some(resume)` before listing.

Validate the requested page count before the worker-enumeration fill loop. It
must be positive, so `count == 0` cannot bypass cursor decoding and return an
empty cursor as `Some("")`. Reject counts greater than `i64::MAX` before they can
reach the Postgres conversion. Apply the same request bounds independently of
the configured backend.

Do not accept the old slash-separated numeric cursor. There is no fallback,
version negotiation, or dual serialization.

## Cursor transition helpers

Replace `SCAN_CURSOR_EPHEMERAL_BIT`, `SCAN_CURSOR_VALUE_MASK`, `cursor_value`,
`scan_modes`, and numeric `next_scan_cursor` with helpers over decoded state.
Keep transitions in `services/oplog/mod.rs` so primary, compressed, blob, and
multilayer services cannot diverge.

Required transitions for one layer are:

1. An empty input cursor starts at layer 0, with the explicitly selected mode or
   durable when both modes are requested, and `resume = None`.
2. `scan_stable` returning `Some(resume)` produces a token for the same layer and
   mode with that resume value.
3. Durable exhaustion while both modes are requested produces a token for the
   same layer, ephemeral mode, and `resume = None`.
4. Exhaustion of the only mode, or of ephemeral in a two-mode scan, returns the
   empty terminal cursor to the enclosing layer service.
5. `MultiLayerOplogService` turns that terminal cursor into a token for the next
   layer's first mode with `resume = None`; exhaustion of the final layer returns
   the empty terminal cursor.

The mode used by `filter_ids_existing_on_lower_layers` must come from the
decoded input state for the page just scanned, not from the next cursor: the next
cursor may already name the following mode.

`is_active_layer_finished()` is no longer needed. Inner-service completion is
the empty cursor; multilayer code either advances it to a non-empty token for the
next layer or lets it remain terminal.

Only `MultiLayerOplogService` knows the configured stack and validates cursor
stack positions in `0..=lower.len()`. Keep stack position distinct from an
archive service's `self.level`, which selects its storage namespace and need not
equal the layer position at which the service is mounted. Primary accepts layer
0. Archive leaves accept the decoded input layer, preserve it while their scan
continues, and return the empty cursor when their final mode is exhausted even
when that input layer is nonzero. Direct archive callers continue to start with
`ScanCursor::default()`.

Within the oplog API, `modes == None` means scan both modes. At the external
listing boundary an absent filter normally resolves to durable-only through
`modes_from_filter`; only all-mode or sufficiently complex filters cause the
two-phase oplog scan.

## Production caller changes

### Primary indexed oplog

In `services/oplog/primary.rs::scan_for_component`:

- decode the input state and require stack position 0
- call `scan_stable` with the decoded `Option<ScanResume>`
- preserve the existing component key prefix and agent-id reconstruction
- use the shared transition helper to encode the continuation
- rename metrics/API labels from `scan` where they specifically identify the
  removed operation

### Compressed indexed archive

Apply the same conversion in
`services/oplog/compressed.rs::scan_for_component`, preserving the archive level
in the indexed-storage namespace and the layer in the continuation state.

### Blob archive and multilayer service

Blob storage does not use `IndexedStorage::scan`, but its enumeration currently
uses the numeric mode helpers. Update `services/oplog/blob.rs` to participate in
the opaque state machine. Its blob listing remains a single page per mode.

Update `services/oplog/multilayer.rs` to decode the input state for layer
dispatch, validate the layer, filter using the page's active mode, and construct
the next-layer token on inner completion. Preserve the current layer order and
duplicate filtering against lower layers. Retain the decoded input state until
the leaf returns: an empty leaf cursor no longer carries the layer needed to
select the next one.

The delegating rate-limited and plugin oplog services should require only type
propagation unless compilation identifies numeric assumptions.

## Public and durable contract propagation

Update every in-tree boundary to transport the token unchanged:

- Change `golem.worker.Cursor` in
  `golem-api-grpc/proto/golem/worker/cursor.proto` to one string field. Update
  the `Cursor`/`ScanCursor` conversions in
  `golem-common/src/model/protobuf.rs` and the executor gRPC request/response
  mapping.
- Let worker-service GET parsing and the CLI `--cursor` parser wrap the opaque
  string. Remove `<layer>/<cursor>` parsing, examples, and error messages.
- Let POST request and response bodies serialize `ScanCursor` as a JSON string.
  Regenerated OpenAPI must show `ScanCursor` as `type: string`.
- Simplify CLI cursor output to `cursor.to_string()`; the value returned to a
  user must be accepted byte-for-byte by the next CLI invocation.
- Change the durable host response payload alias
  `AgentsPage` from `Option<(u64, u64)>` to
  `Option<ScanCursor>`. Pass it directly rather than converting through tuples.
  `get-agents` keeps the cursor hidden in `GetAgentsEntry`, so its WIT interface
  and SDK-facing iteration API do not change.
- Update all mocks, test DSLs, fixtures, and literal `ScanCursor` constructors.

Changing `AgentsPage` changes the persisted durable-call response shape for
guest `get-agents`. Per repository policy, update it directly without legacy
payload decoding. It is not part of the deployment diff model and does not
require a `DIFF_MODEL_VERSION` bump.

## Remove the positional scan

After both indexed oplog callers compile against `scan_stable`:

1. Delete `IndexedStorage::scan` from the trait.
2. Delete `IndexedStorageLabelledApi::scan`.
3. Delete the old implementation from memory, Postgres, SQLite, Redis, and
   multi-SQLite.
4. Redis `scan_stable` currently delegates to its old `scan`; move the Redis
   `SCAN`/composite-key logic into `scan_stable` or a private backend helper
   before deleting it.
5. Remove the storage-level `ScanCursor = u64` alias if it has no remaining use;
   `ScanResume::Cursor` can carry `u64` directly.
6. Remove obsolete mock trait methods from `services/oplog/tests.rs` and
   `services/oplog_sweep.rs`.
7. Replace the generic `scan_*` indexed-storage tests with stable-scan versions
   rather than dropping their namespace, prefix, empty-result, and pagination
   coverage. Retain the deletion-during-walk and multi-SQLite-specific tests
   introduced by PR #3802.
8. Update non-`scan_*` tests that call the removed method, including
   `postgres_singleton_append_many_preserves_storage_contract`.

A final source search for `.scan(` must show no indexed-storage scan call or
implementation. Unrelated methods named `scan`, such as oplog cut-point scans,
remain untouched.

## Tests

### Cursor codec unit tests

Add focused tests for:

- marker and Redis cursor round trips, including punctuation and Unicode in a
  marker
- initial default state
- durable continuation, durable-to-ephemeral transition, and ephemeral
  exhaustion
- next-layer transition with `resume = None`
- terminal cursor conversion to `None`
- malformed prefix/base64/JSON, unknown version/variant, incompatible mode,
  invalid layer, embedded-NUL marker, and backend resume mismatch returning an
  error rather than panicking
- zero and greater-than-`i64::MAX` page counts being rejected before cursor or
  storage processing
- public `Display`/`FromStr`, serde JSON, binary payload, and protobuf round
  trips preserving the opaque token exactly

### Indexed storage tests

Run the existing test matrix against all five backends for:

- empty scans
- single-page and multi-page scans
- prefix filtering
- duplicate index rows producing one key
- deletion behind the continuation not skipping surviving keys

Keep the multi-SQLite file-boundary and newly-created-file tests. Do not assert
global uniqueness for Redis; its contract permits duplicate keys.

### Oplog enumeration tests

Adapt and strengthen the existing tests for:

- explicit durable-only and ephemeral-only scans
- both-mode pagination crossing the durable/ephemeral boundary
- primary-to-compressed-to-blob layer transitions
- direct compressed-archive and direct blob-archive drains from the default
  cursor
- empty namespaces and empty intermediate pages terminating
- filtering duplicates that already exist in lower layers
- a page resumed from a serialized/deserialized public token
- invalid tokens reaching the executor through gRPC as invalid requests

Use enough agents and a small page size so each test actually crosses the
boundary it claims to cover. Compare the complete set of agent IDs and check the
expected mode/layer progression by privately decoding returned tokens in module
tests.

### Durable host and external surfaces

Add a targeted `get-agents` durable-host regression: record a page with
`Some(opaque_cursor)`, reconstruct/replay the caller while preventing a new live
enumeration call, and verify that the resource's subsequent live page resumes
with the recorded token. Existing live enumeration tests do not prove cursor
restoration on replay. Update worker-service and CLI tests to verify that a
cursor printed from one bounded listing can be supplied unchanged to retrieve
the next page.

For malformed-cursor gRPC coverage, assert the domain
`Failure(InvalidRequest)` response envelope. The endpoint reports worker errors
inside a successful tonic response rather than necessarily returning
`Status::invalid_argument`.

## Generated artifacts

The public JSON shape changes, so regenerate and commit:

- `openapi/golem-worker-service.yaml`
- `openapi/golem-service.yaml`
- `docs/src/content/next/rest-api/*.mdx`

Use `cargo make generate-openapi`, which also regenerates the REST API docs.
`golem-client` uses the shared `golem_common::model::ScanCursor`; verify its build
after regeneration. Protobuf Rust is generated at build time, so update the
`.proto` source and compile all consumers rather than checking in ad hoc
generated Rust.

The old numeric cursor also appears in generated-skill source material. Update
the `--scan-cursor 0/5` example in:

- `golem-skills/skills/common/golem-list-and-filter-agents/SKILL.md`
- the corresponding embedded skill in
  `sdks/moonbit/golem_sdk_example1/.agents/skills/`

Then run `cargo make generate-docs-skills` and commit the regenerated
`docs/src/content/next/how-to-guides/` output. Verify with
`cargo make check-docs-skills`. The CLI structured-output schema already models
cursor-map values as arbitrary strings and needs no shape change.

## Verification sequence

Use the `adding-dependencies`, `modifying-http-endpoints`, and `testing` skills
while implementing. Run the smallest checks first:

1. `cargo fmt --all -- --check`
2. Cursor codec and oplog helper unit tests in `golem-common` and
   `golem-worker-executor`.
3. The affected `golem-worker-executor` indexed-storage test filters, including
   backend matrix cases available in the orb.
4. `cargo test -p golem-worker-executor --lib -- scan_for_component --report-time`
   or the equivalent test-r filter established by the testing skill.
5. The `oplog_blob_archive` integration-test target and targeted durable-host
   `get-agents` live/replay tests.
6. Package-scoped checks/builds for `golem-common`, `golem-api-grpc`,
   `golem-worker-executor`, `golem-worker-service`, `golem-client`, and
   `golem-cli`.
7. `cargo make generate-openapi`, followed by `cargo make check-openapi` and
   `cargo make check-docs-openapi`.
8. `cargo make generate-docs-skills`, followed by
   `cargo make check-docs-skills`.
9. Targeted CLI integration pagination coverage if its required services are
   available.

Do not run `cargo make test`. If a real Postgres, Redis, multi-SQLite, MinIO, or
CLI integration dependency is unavailable, run every in-process affected test
and report the unexecuted backend separately.

## Completion criteria

- Agent listing uses `scan_stable` for primary and compressed indexed oplogs.
- Public clients see and round-trip only an opaque string cursor.
- Durable/ephemeral and layer transitions preserve current listing coverage.
- Invalid external tokens produce request errors and cannot panic the executor.
- `IndexedStorage::scan`, all five implementations, wrappers, mocks, tests, and
  OFFSET SQL are gone.
- OpenAPI, docs, protobuf consumers, CLI, worker service, guest durable payload,
  and tests all use the new contract.
- Both REST request and response models, HTTP/gRPC test DSL consumers, and the
  literal cursor in `integration-tests/tests/plugins.rs` use the opaque type.
- Targeted checks pass, with any unavailable infrastructure-backed checks called
  out explicitly.
