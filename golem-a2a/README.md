# Golem A2A wire codec

This crate targets A2A v1.0.1. It uses the released `a2a-lf` 0.4.3 types and
`a2a-pb` 0.4.0 ProtoJSON codec. It does not install routes, implement protocol
operations, own a task store, or send HTTP requests.

The API consists of free functions:

- `wire::{to_value, from_value}` handles protocol payloads. Do not serialize
  the native `a2a` types with plain serde for wire traffic.
- `jsonrpc` re-exports upstream envelopes and method names. `parse_request`
  returns an error response with a null ID for an unparseable JSON body.
- `http::ROUTES` and `http::match_route` describe exactly §5.3. The matcher
  takes a path without its query and returns escaped ID segments. The HTTP
  boundary must decode captures after matching, not before identifying actions.
- `http::{encode_error, decode_error}` handles §11.6 error bodies. Encoding
  includes the `status` name; decoding also accepts its omission and uses the
  numeric HTTP code as a final fallback. Both bindings require error details
  to have a nonempty string `@type` and preserve unknown typed payloads.
- `stream::{encode_http, decode_http, encode_jsonrpc, decode_jsonrpc}` handles
  the distinct stream envelopes. JSON-RPC decoding requires the original ID
  and returns `DecodeError::Remote` for remote errors, including null-ID parse
  and invalid-request errors. Other mismatched IDs remain invalid.
- `stream::{sse_event, sse_response, parse_sse, sse_data}` uses Poem framing and
  the existing `sse-stream` parser. HTTP+JSON's named SSE `error` extension is
  not defined by §11.7 and is rejected, not treated as a protocol event.

Subscription uses POST, as specified in §5.3 and §11.3.2. The v1.0.1 proto's
HTTP annotation instead uses GET. This crate follows the prose route table;
clients that use the proto's GET subscription binding are not supported.
Wrong-method action paths do not match `GetTask`. Colons inside IDs must be
escaped so they cannot be confused with action suffixes.

The REQUIRED-field defaults and traversal live together in `src/wire.rs`.
Traversal follows known protocol fields and never rewrites keys inside metadata
or `Part.data`. Nested cards, tasks, messages, artifacts, statuses, and events
receive the same normalization as top-level values. Encoding a required empty
array preserves its presence; it does not make that array semantically valid.
Validation of protocol operation semantics belongs to the protocol core.

The [fixture notes](tests/fixtures/README.md) explain the specification examples
that are illustrations rather than valid wire payloads and how tests distinguish
their discrepancies from the normative contract.
