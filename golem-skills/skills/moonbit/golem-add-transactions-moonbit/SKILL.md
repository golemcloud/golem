---
name: golem-add-transactions-moonbit
description: "Implements saga-pattern compensation in MoonBit Golem agents. Use when a workflow needs reverse-order compensation, domain failure reporting, or another attempt after compensation."
---

# Saga-Pattern Transactions (MoonBit)

The MoonBit SDK has no high-level saga runner. Implement the workflow with ordinary guest
control flow and durable host calls. Do not change the execution cursor to retry business work.

## Compensate and loop

The following is application pseudocode, not an SDK API:

```text
repeat:
  completed = []
  for each step:
    result = step.execute()
    if result succeeded:
      register compensation with the successful result
    else:
      run completed compensations in reverse order
      if any compensation failed:
        return partial-compensation failure with the original error
      if failure is acceptable:
        return the original domain error
      otherwise:
        resolve the saga policy and request next-retry-delay with continuation state
        if exhausted or invalid, return failure preserving the original error
        await the delay with a durable timer, then start another attempt
  if every step succeeded:
    return success
```

Register compensation only after a step succeeds. Preserve compensation failures so callers do
not mistake partial compensation for a completed rollback. A permanent domain failure can loop
forever; prefer a fallible workflow unless the application has an explicit retry decision.
The SDK exposes `@api.resolve_retry_policy` and `@api.next_retry_delay`, not a saga runner.
An application can resolve verb `saga`, noun the agent URN, with `attempt`/`error-kind` after
complete compensation. Pass the opaque binary continuation unchanged, measure elapsed work and
waiting with durable clocks, and await delays with durable timers. No matching policy selects
the agent default. Unbounded policies are rejected; exhaustion and policy errors must return a
failure preserving the original error. Do not invent a MoonBit saga helper or reset the budget.

## Recovery rules

- Each new attempt appends new calls. Failed attempts and compensations remain in the oplog.
- Reconstruction replays completed call outcomes and repairs an eligible incomplete call at its
  own `Start`; it does not rerun the whole workflow.
- Use domain `Result` values for expected failures. A deterministic guest trap fails the
  invocation and does not guarantee compensation.
- Make execute and compensation actions duplicate-safe. External exactly-once behavior requires
  peer-enforced idempotency keys.
- Host HTTP transport, status-code and response-body retries remain host responsibilities. They
  are separate from the application's compensate-and-loop decision.
- A lost nonresumable handle reports typed loss. Explicitly reopen and initialize the next
  session; loss does not prove an earlier write never reached the peer.
