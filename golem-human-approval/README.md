# Human approval tool middleware

This package contains the durable decision service used by the reusable `human-approval`
universal tool middleware. The middleware implementation is built by the
`golem-it:tool-streaming-rust-middleware` component in
`test-components/tool-streaming/rust-middleware` and can be published as an ordinary tool
middleware release.

## Contract

Each middleware invocation generates and durably records one UUID `requestId` and one owner-scoped
Golem promise. The complete immutable registration tuple is:

`(requestId, owner component/agent, promise oplog index, policy, tool, command path, principal kind)`.

Re-registering the same tuple is idempotent. Reusing a request ID with different data is a
conflict. The service persists a transition before completing the Golem promise:

```text
pending ── approve ──▶ approved
        ├─ deny ─────▶ denied
        ├─ cancel ───▶ cancelled
        └─ reconcile owner loss ──▶ abandoned
```

Decision idempotency is `(requestId, owner, terminal state)`: repeating the same terminal state
retries an unfinished promise callback and retains the first accepted `decidedBy` audit value.
Every different late terminal state returns HTTP 409. A successful approval is the only state that
lets the middleware admit the pinned next layer. Denial never dispatches. Cancellation completes
the wait with a cancellation error. Owner termination must be reconciled through `abandon`; a
later approval then conflicts and never produces a callback. Dropping a caller's result observer
does none of these things: the durable owner invocation remains pending and can still execute
after an approval.

The request token authenticates middleware registration. The separate decision token authenticates
operator decisions, reads, and owner-loss reconciliation. A decision must repeat the exact owner
identity stored in the request. Use HTTPS and inject all three tokens from a secret store; do not
put token values in middleware parameters or manifests. The service records only the principal
kind (`anonymous`, `oidc`, `agent`, or `golem-user`), never OIDC claims or tool inputs.

## Run the service

```shell
export GOLEM_APPROVAL_REQUEST_TOKEN='...'
export GOLEM_APPROVAL_DECISION_TOKEN='...'
export GOLEM_APPROVAL_STORE=/var/lib/golem-approval/requests.json
export GOLEM_APPROVAL_LISTEN=0.0.0.0:9090
export GOLEM_API_URL=https://api.golem.example
export GOLEM_API_TOKEN='...'
cargo run -p golem-human-approval --bin golem-human-approval-service
```

`GOLEM_APPROVAL_STORE` is atomically replaced and fsynced after every transition. Run one service
writer per store file. The Golem API token needs permission to complete promises for protected
owners. If promise completion returns 404 or 410, the record is retained with callback state
`ownerGone`; the already-persisted decision remains terminal and no leaf can dispatch. Reconcile
known owner loss with `abandon` while the request is still pending.

Operator endpoints require `Authorization: Bearer $GOLEM_APPROVAL_DECISION_TOKEN`:

- `GET /v1/requests` and `GET /v1/requests/{requestId}` list/read requests.
- `POST /v1/requests/{requestId}/decision` accepts an `ApprovalDecision` with the stored owner,
  `state` (`approved`, `denied`, or `cancelled`), and a safe operator identifier in `decidedBy`.
- `POST /v1/requests/{requestId}/abandon` accepts the stored owner after authoritative owner-loss
  detection.

## Bind the middleware

Build `test-components/tool-streaming`, publish the exported `human-approval` definition as a
middleware release, and install it like any other universal middleware. Its parameters are public
configuration; the registration credential is supplied to the calling agent as the
`GOLEM_HUMAN_APPROVAL_REQUEST_TOKEN` environment variable.

```yaml
tools:
  middleware:
    human-approval:
      release:
        account: middleware@example.com
        name: human-approval
        version: 1.0.0

agents:
  DeploymentAgent:
    tools:
      deploy:
        middleware:
          - name: human-approval
            parameters:
              requestUrl: https://approval.example/v1/requests
              policy: production-change
```

The exact release/publication syntax and effective middleware ordering are documented in
`docs/src/content/next/app-manifest.mdx`.
