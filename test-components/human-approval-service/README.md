# Human-approval test support

This crate supplies the HTTP decision-service fixture used by automated human-approval middleware
tests. The middleware itself lives in `test-components/tool-streaming/rust-middleware`. Neither is
a built-in or production service, and neither is provisioned at registry startup. There is no
standalone service binary or publication workflow.

The fixture retains requests and callback locks indefinitely, rewrites its complete JSON store on
each transition, and returns all records when listing. Those choices support finite, isolated test
workloads; they are not a production storage or retention design.

## Tested contract

Each middleware invocation durably records a UUID and an owner-scoped Golem promise. Registration
includes the owner, promise oplog index, policy, tool, command path, and principal kind. Repeating
the same registration is idempotent; changing its data under the same request ID conflicts.

Requests transition from pending to approved, denied, cancelled, or abandoned. Only approval lets
the middleware dispatch the next layer. A terminal decision is persisted before promise completion.
Repeating the same decision retries unfinished delivery; a conflicting late decision is rejected.
Callback delivery handles owner loss without dispatching the leaf. Dropping the result observer does
not cancel the durable owner invocation.

Separate registration and decision tokens exercise authentication boundaries. The fixture records
the principal kind, not OIDC claims or tool inputs. Tests start the router with temporary stores,
provide decisions programmatically, and shut it down; no test needs a human to approve a request.

## Tests

```sh
cargo test -p golem-human-approval --lib -- --report-time
```

Service acceptance tests live in `src/acceptance_tests.rs`. Deployed middleware coverage lives in
`golem-worker-executor/tests/tool_streaming/middleware_acceptance.rs`. The support crate remains a
root-workspace member so its tests run in the ordinary unit-test suite, and only the worker
executor's test dependencies consume it.
