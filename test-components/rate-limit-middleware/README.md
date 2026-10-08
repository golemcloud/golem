# Persistent rate-limit test middleware

This component is a test-only fixture for universal middleware admission, shared durable state,
and replay. It is not a built-in, is not published or provisioned at registry startup, and is not
intended for production reuse. Its windows, decisions, and charged invocation IDs are retained
indefinitely; the tests use finite, isolated workloads rather than a production retention policy.

`test-components/tool-streaming/golem.yaml` builds this source as
`golem:rate-limit-middleware` and copies `golem_rate_limit_middleware_release.wasm` for worker
executor tests. The artifact suffix does not imply a published release.

## Tested contract

Each fresh middleware instance calls a durable `RateLimitBackend` agent identified by
`(policy, limit, windowMilliseconds)`. Different owners acting as the same authenticated principal
share capacity within that configuration. Different principals and configurations have independent
counters. Anonymous invocations share the `anonymous` key.

The fixed windows align to Unix epoch boundaries. Exactly `limit` new logical invocations are
admitted per window; subsequent calls return `resource-exhausted` before dispatch. Each invocation
durably generates a UUID. The backend remembers admitted and rejected decisions so replay returns
the original decision without an extra charge. Configuration changes select a separate backend;
an in-flight invocation retains its pinned configuration during recovery.

## Tests

```sh
cargo make test-component-middleware-unit-tests
```

Deployed coverage lives in
`golem-worker-executor/tests/tool_streaming/k3_persistent_rate_limit_acceptance.rs` and
`middleware_acceptance.rs`. It exercises principal isolation, concurrent owners, configuration
changes, and crashes before or after leaf dispatch. Build through the `tool-streaming` application
using the normal test-component workflow before running those tests.
