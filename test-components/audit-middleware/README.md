# Audit test middleware

This is a test-only fixture, not a built-in or a production middleware release. It is built and
copied by the normal test-component workflow and is not provisioned at registry startup.

`audit` is a universal tool middleware that emits one versioned JSON record after the underlying
tool and its declared stdout/stderr streams settle. It forwards inputs, host-managed resources,
results, errors, and stream bytes unchanged.

The middleware records tool and command identity, the calling owner, the runtime principal,
per-occurrence `label`, safe value-shape summaries, stream byte/chunk counts and terminals. Value
summaries contain only kinds, collection sizes, string/binary lengths, and counts of opaque
secret/quota/permission/stream resources. They never contain scalar values, secret identities,
secret paths, custom-error payloads, error messages, or stream bytes.

## Sink contract

The sink must accept `POST` with `content-type: application/json` and deduplicate by the
`idempotency-key` header. That header equals `policyInvocationKey` in the body. The key is durably
committed before the underlying call and remains stable across replay and HTTP retry. The
deduplication namespace is the configured sink endpoint: commit at most one record for each
`(endpoint, policyInvocationKey)`. Delivery attempts may repeat. Different middleware occurrences,
including duplicate `audit` entries, intentionally have different keys and are distinguished by
`label`.

The middleware is fail-closed: a transport failure or non-2xx response fails the middleware call.
A 2xx response acknowledges a committed record. Sink implementations must not acknowledge before
the deduplicated record is durable.

## Build and test

```sh
golem build -P release --force-build --yes
golem exec -P release copy
```

The manifest copies `golem_audit_middleware_release.wasm` into the parent `test-components`
directory. The artifact suffix does not imply a published release. Automated worker executor tests
install it directly, provide an in-process sink, and exercise duplicate occurrences, stream
transparency, identity attribution, and replay deduplication.

```sh
cargo make test-component-middleware-unit-tests
```

Audit needs no config or secret capability. Keep both secret scopes empty; opaque handles still
pass to an independently authorized inner tool.
