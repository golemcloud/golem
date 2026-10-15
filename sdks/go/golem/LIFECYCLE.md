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
untaken outputs.

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
