# Results and tool outputs

`tool.Invocation.Detach` permanently stops local result delivery. A later `Wait` returns
`ErrResultDetached`. Detach does not cancel the invocation or close either tool
output. It releases the result observer immediately unless a host Get is active.
In that case an already blocked Wait and physical release wait for Get to settle;
delivery is marked detached immediately, but the SDK does not interrupt Get.
Late success values and
custom tool-error payloads are disposed in their raw form before decoding or
transferring capabilities. Capabilities already delivered belong to the caller;
detaching does not revoke them. Concurrent waiters share one observation and see
the same completed outcome, not an intermediate zero value.

`tool.Invocation.Cancel` explicitly requests invocation cancellation. It does not detach the
result or close local output readers. The runtime can complete the invocation
before cancellation takes effect. Cancel after physical observer release is a
no-op. Cancel during a detached but still active Get can still reach the runtime.

Closing stdout or stderr is permanent and independent of its sibling and of the
invocation. `Close` returns promptly while an active native read continues. The
SDK suppresses late bytes and releases the reader after that read settles. It
does not interrupt the native read, promise resumable pending reads, or install a
new buffering mechanism. An active read that never settles can retain its host
resource. A taken output must be read or closed by its owner; Wait drains only
untaken outputs. After detachment, Wait still starts those drains (or forwards
untaken outputs beneath middleware), but returns `ErrResultDetached` without
waiting for them. Detach alone does not start output drains. Take an output
before Wait to retain control of reading or closing it. After Detach, the caller
must still call Wait, or take and read or close each output. An unread output
can retain its host resource and block the producer on backpressure. Detach is
result-only, not a fire-and-forget operation for the whole call.

Tool handlers and middleware use the same output terminal rules:

| Handler outcome | Invocation outcome | Implicit outputs |
| --- | --- | --- |
| Successful return | Success | Attempt finish independently |
| Declared RaisedError, returned or panicked | Custom error | Attempt finish independently |
| Undeclared named RaisedError | Invalid result | Attempt finish independently |
| Intentional SDK error or forwarded tool error | Preserve its category | Attempt finish independently |
| Ordinary panic or unclassified returned error | Component trap | Attempt fail independently, then panic unchanged |

An implicit finish error or cleanup panic never replaces the handler outcome,
skips cleanup of its sibling, or overwrites an explicit output terminal.

## Compensable transactions

`transaction.Fallible` and `transaction.Infallible` compensate successful steps in
reverse order after a body error or a catchable panic. A failed or panicking step
does not register its compensation. Compensation stops at its first error or panic.
`Fallible` returns a `Failure` after a returned error. After a panic it compensates,
then re-panics with the original value only after complete rollback. Partial rollback
returns a `Failure` that preserves the original panic and the compensation failure.

`Infallible` returns `golem.Result[Out, transaction.Failure[E]]`, not a bare `Out`.
Handle its error arm: a partial rollback, an exhausted retry policy, or a policy
error must not be ignored. `Failure.Error` holds the returned body error;
`Failure.Panic` holds the original recovered value instead when the body panicked.
`CompensationFailure` and `CompensationPanic` preserve rollback failures.
`RolledBackFully` accounts for both kinds of rollback failure.

After a complete rollback, `Infallible` resolves a policy with verb `saga`, the
current agent's `urn:worker:<component-id>/<agent-id>` URI, a one-based `attempt`,
and `error-kind` (`operation-error` or `panic`). No transaction name is available
in this API. It passes that same context and the opaque retry state to the host
evaluator. The first state is empty; subsequent decisions receive the returned
state. The host uses its configured retry policy when no rule matches. A saga
policy must have a finite retry count or time bound, or use `Never`; policy changes
and invalid policies return `Failure.PolicyError`. Exhaustion returns
`Failure.Exhausted`. Partial rollback never starts another attempt.

The durable clock is observed before the first attempt. Elapsed time includes
the body, rollback, and each durable `time.Sleep`. Saga retries use the host
evaluator, not `retry.Policy.Schedule`. Attempts append history; they do not
rewind previous calls. A process abort or an uncatchable runtime trap cannot run
Go compensation code.
