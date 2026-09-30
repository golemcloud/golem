# Audit tool middleware

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

## Build and publish

```sh
golem build -P release --force-build --yes
golem exec -P release copy
golem deploy -e local --yes
```

The included `golem.yaml` publishes `audit@0.1.0` from the local environment. A consuming
application can bind the published release more than once with independent configuration:

```yaml
tools:
  middleware:
    audit:
      release:
        account: builtin-tool-owner@golem.cloud
        name: audit
        version: "0.1.0"

environments:
  production:
    server: cloud
    tools:
      middleware:
        - name: audit
          parameters:
            label: security
            sinkUrl: https://audit.example/v1/records
          secretKeysReadable: []
          secretKeysRevealable: []
        - name: audit
          parameters:
            label: operations
            sinkUrl: https://audit.example/v1/records
          secretKeysReadable: []
          secretKeysRevealable: []
```

Audit needs no config or secret capability. Keep both secret scopes empty; opaque handles still
pass to an independently authorized inner tool.
