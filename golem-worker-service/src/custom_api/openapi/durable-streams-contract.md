# Durable Streams deployment OpenAPI contract

The deployment OpenAPI document describes the compiled route and the behavior of
`custom_api/durable_streams.rs` and `custom_api/durable_streams/append.rs`. It is
not the static management-service OpenAPI document. Unsupported protocol options
must not be advertised as implemented operations.

## URL and operation matrix

| Resource | Method | Success | Request / response |
| --- | --- | --- | --- |
| Method base | PUT | 201 (200 for replay) | Compiled non-stream request body and arguments; optional expiry policy; empty response with session Location |
| Invocation session | PUT | 201 / 200 | Same compiled body, arguments and optional expiry policy; empty response; conflicting invocation identity or policy is 409 |
| Invocation session | HEAD | 200 | No body; JSON Content-Type, Cache-Control: no-store, Stream-Closed and configured expiry policy |
| Invocation session | GET | 200 | Manifest: session, streams, closed |
| Invocation session | DELETE | 204 | Cooperative cancellation; repeated cancellation succeeds; history remains readable |
| Concrete stream slot | PUT | 201 / 200 | Empty body only; optional matching Content-Type; stream metadata response headers |
| Concrete stream slot | HEAD | 200 | No body; stream metadata headers |
| Concrete stream slot | GET | 200 / 204 / 304 | Catch-up, long-poll, or SSE, selected by query parameters |
| Concrete input slot | POST | 204, or 200 / 204 with producer headers | Typed JSON messages or bytes; optional atomic closure; metadata and producer response headers |
| Concrete stream slot | DELETE | 204 | Cancel and tombstone the slot; repeat returns 410 |

Output slots do not advertise POST. A runtime attempt returns 405 with Allow.
Unknown sessions/slots return 404. Tombstoned slots return 410 for HEAD, GET,
POST and DELETE, and 409 for PUT. Invalid requests return 400; unavailable
executor routing returns 503 with Retry-After. Authentication remains the
compiled route's authentication policy.

Slot PUT can create a session only when all non-stream method parameters are
bound from path/query. Otherwise clients must PUT the session first. Lazy POST
has the same prerequisite and returns a 404 problem response when it is unmet.
Missing-session slot PUT instead returns 400 in that case.
Session creation bodies follow RequestBodySchema: JSON, text, binary, or absent;
they are not necessarily JSON argument objects. Base/session PUT consumes those
arguments; session and slot GET/HEAD/DELETE do not. Existing-slot POST does not
consume them, while lazy POST conditionally consumes URL-bound arguments. Slot
PUT revalidates method arguments only when every parameter is Path or Query;
with mixed body/header parameters it does not validate query arguments either.
Do not make method arguments universally required on these operations.

The family is `<base>`, `<base>/invocations/{session}`, and
`<base>/invocations/{session}/streams/<literal-slot>`. Session IDs match
`^[A-Za-z0-9._-]{1,128}$`; generated IDs are ULIDs but callers may use other IDs.
Resolve schema references before deriving slots:

| Compiled schema | Slots |
| --- | --- |
| User-supplied top-level input stream<T> | Input slot named after the parameter, element T |
| Direct output stream<T> | Output $result, element T |
| Output record containing streams | Every field must be a direct stream; one output slot per field, no aggregate $result |
| Non-stream single output T | Output $result containing the whole T, including non-stream records |
| Unit output | No output slot |

Only direct stream<u8> uses octet-stream. Scalar u8 results still use JSON.
Do not apply REST option/result status or text/binary response lowering to slots.

## Read representations

- Only offset, live and cursor are protocol query parameters in the handler.
  Do not advertise max-bytes, max-items or a client-selectable timeout.
- Offset defaults to -1, including in live modes. Requests accept -1, now, or
  48 lowercase hexadecimal characters; returned offsets are hexadecimal only.
  The runtime also validates the offset version and reserved bits.
- live is long-poll or sse. cursor is an unsigned decimal integer bounded by
  u64::MAX - 180. Repeated protocol query parameters are invalid.
- JSON catch-up/long-poll data is an array of the slot's element type. A direct
  stream<u8> uses application/octet-stream. SSE uses text/event-stream, with
  Stream-SSE-Data-Encoding: base64 for byte streams.
- Long-poll with no items returns 204. Closed-stream catch-up can return 304
  for a matching If-None-Match. HEAD has no response body.
- Metadata includes Stream-Next-Offset, Stream-Closed, Stream-Cancelled,
  Stream-Up-To-Date and Cache-Control as applicable. Long-poll responses include
  Stream-Cursor, including on 204; SSE has no HTTP Stream-Cursor header.
  Data responses can include ETag. Live-reader admission can
  return 503 with Retry-After; catch-up admission can return 429.
- SSE data events contain a JSON array or base64-encoded byte batch and are
  omitted for empty batches. Control events contain streamNextOffset (string)
  and upToDate (boolean), plus streamClosed: true when closed and up-to-date,
  otherwise streamCursor (string). Base64 encoding applies only to data events.
- Manifest stream entries contain name, contentType, nextOffset, closed,
  cancelled and deleted. The manifest does not include stream data.

## Append representations

- JSON arrays flatten exactly one level. A non-array value is one message;
  an array is a batch of messages. Array-valued elements require an outer
  array. Empty JSON batches are invalid. The batch limit is 4096 messages.
- A nonempty body requires the slot Content-Type; mismatch is 409. Empty bodies
  are accepted only with Stream-Closed: true and ignore Content-Type.
- Stream-Closed is case-insensitive true; other values are treated as absent.
- Producer-Id, Producer-Epoch and Producer-Seq are optional as a group. Epoch
  and sequence are integers from 0 through 9007199254740991. Newly accepted
  producer writes return 200 and duplicates 204. Stale epochs return 403 with
  Producer-Epoch; gaps return 409 with Producer-Expected-Seq/Received-Seq.
- Successful writes report Stream-Next-Offset and Stream-Closed; producer
  successes also report Producer-Epoch and Producer-Seq. Closed-stream conflicts
  report final offset and closure. Body limits return 413, rate limits 429.
- Append validation can return application/problem+json with path and detail;
  many other errors have no body. Do not require a problem body on every error.

## Document and browser invariants

Emit concrete slot paths so each operation has the correct element schema,
media types and direction. Preserve path/query/header bindings and use unique
path parameter names for the generated session capture. Merge named schemas
through the existing schema graph and preserve ordinary REST operations.

Use x-golem-route-mode: durable-streams and x-golem-stream-slot metadata.
Operation IDs must be stable and unique across families, slots and methods.
Stream-TTL is a canonical non-negative decimal sliding idle timeout in seconds;
Stream-Expires-At is a future RFC3339 timestamp. They are mutually exclusive,
duplicates are invalid, and HEAD reports the configured policy without touching
the deadline. Sliding activity means a new origin GET or an accepted/duplicate
POST append. Repeated PUT, HEAD, continuation reads within long-poll/SSE, bytes
flowing on an already-open response, and the agent's own production do not refresh
the deadline. Refreshes are coalesced until they move the deadline by at least 10%
of the TTL. Closed historic pages are cacheable only up to the remaining
expiry lifetime and require revalidation; all other expiring responses are
no-store. Do not emit subscriptions, SDK opt-outs or annotation overrides that
do not exist.

Public session IDs are stable URL identities, not invocation idempotency keys.
Durable creation binds the public ID to a fresh invocation key; expiry retires that
binding, and a later explicit PUT may recreate the same URL with another key.
Ephemeral sessions are fail-stop and cannot be recreated. Export-fork retries use
the immutable target creation receipt, so an already-created target remains
discoverable after the source advances, expires, is tombstoned or is deleted.

Fork sessions use `<base>/forks/{fork}/invocations/{session}` and expose GET/HEAD.
Their concrete slot paths expose PUT with Stream-Forked-From, optional
Stream-Fork-Offset/Stream-Fork-Sub-Offset, an optional typed initial body and
Stream-Closed. Creation returns 201, matching retries 200, with Location;
conflicts return 409, copy/body limits 413 and admission limits 429 with
Retry-After when available. Initial content or closure on a read-only slot
returns 403. GET/HEAD/DELETE and writable-slot POST use the ordinary stream
contract, but fork POST never lazily creates a session. The manifest includes
nullable fork provenance (sourcePath, forkOffset and subOffset); fork provenance
is not emitted as response headers.

Browser preflight must allow the supported producer and closure request headers;
responses must expose producer outcome and expiry headers alongside the read metadata.
Origin and credential policies remain unchanged.

Deployment-spec snapshots and an actual generated-client integration test cover
this contract. Static generate-openapi/check-openapi tasks do not exercise it.
