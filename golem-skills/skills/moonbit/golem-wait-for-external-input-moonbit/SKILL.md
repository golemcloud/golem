---
name: golem-wait-for-external-input-moonbit
description: "Waiting for external input using Golem promises in a MoonBit Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (MoonBit)

## Overview

A **Golem promise** lets an agent suspend its execution until an external event completes it. The agent creates a promise, passes the promise ID to an external system (another agent, a webhook, a UI, an HTTP API call), and then awaits the result. Creation does not suspend the agent. For durable agents awaiting a pending promise, suspension occurs after the suspension policy delay and frees guest execution resources; persisted state and other resources remain.

## API

All functions are in the `@api` package of the Golem MoonBit SDK:

| Function | Signature | Description |
|----------|-----------|-------------|
| `@api.create_promise` | `() -> PromiseId` | Creates a new promise and returns its ID |
| `@api.await_promise` | `async (PromiseId) -> Bytes` | Suspends until the promise is completed |
| `@api.complete_promise` | `(PromiseId, Bytes) -> Bool` | Completes a promise with a byte payload |
| `@api.get_promise` | `(PromiseId) -> PromiseResult` | Gets an owned handle for async result retrieval |

## Usage Pattern

### 1. Create a Promise and Wait

Run this fragment inside an `async fn`. Any helper or agent method that waits,
and every caller of that helper, must also be async.

```moonbit
let promise_id = @api.create_promise()
// Pass promise_id to an external system...

// Awaiting can suspend the durable agent after the suspension policy delay
let data : Bytes = @api.await_promise(promise_id)
```

### 2. Complete a Promise from Another Agent

```moonbit
let payload = "approved"
@api.complete_promise(promise_id, @utf8.encode(payload[:]))
```

### 3. Advanced: Own the Result Handle

Run this fragment inside an `async fn`. There is no `subscribe` or optional synchronous polling API. Release a directly acquired handle, including when the async operation exits with an error:

```moonbit
let promise_result = @api.get_promise(promise_id)
defer promise_result.drop()
let data : Bytes = promise_result.get()
```

## PromiseId Structure

A `PromiseId` contains an `agent_id`, a `fingerprint` and an `oplog_idx`. The `fingerprint` is a `@types.Uuid` that identifies the agent incarnation that created the promise: a deleted and re-created agent keeps its agent ID but gets a new fingerprint, so a promise ID of the deleted incarnation does not reach the new one. Golem displays a promise ID as `<agent-id>/<fingerprint>/<oplog-idx>`.

To let an external system complete the promise via the Golem REST API, the agent must expose the agent ID, the fingerprint and the oplog index. `@schema.Uuid::from_wit(promise_id.fingerprint).to_string()` gives the fingerprint as a UUID string. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json
Authorization: Bearer <token-with-agent-invoke-permission>

{"oplogIdx": <oplog_idx>, "fingerprint": "<fingerprint-uuid>", "data": [<bytes>]}
```

All three body fields are required.

## Full Example: Human-in-the-Loop Approval

```moonbit
/// The values an external caller needs to complete the approval promise
#derive.golem_schema
pub(all) struct ApprovalRequest {
  oplog_idx : UInt64
  fingerprint : String
}

#derive.agent
struct WorkflowAgent {
  name : String
  mut pending : @types.PromiseId?
  mut last_result : String
}

fn WorkflowAgent::new(name : String) -> WorkflowAgent {
  { name, pending: None, last_result: "" }
}

/// Start an approval workflow that waits for external input
pub fn WorkflowAgent::start_approval(self : Self) -> ApprovalRequest {
  let promise_id = @api.create_promise()
  self.pending = Some(promise_id)
  // Return the oplog index and the fingerprint so the external caller can complete the promise
  {
    oplog_idx: promise_id.oplog_idx,
    fingerprint: @schema.Uuid::from_wit(promise_id.fingerprint).to_string(),
  }
}

/// Wait for the approval to arrive and return the result
pub async fn WorkflowAgent::wait_for_approval(self : Self) -> String {
  guard self.pending is Some(promise_id) else { return "No approval is pending" }
  let data = @api.await_promise(promise_id)
  self.pending = None
  let result = @utf8.decode_lossy(data[:])
  self.last_result = result
  result
}
```

## Use Cases

Import `"moonbitlang/core/encoding/utf8" @utf8`, `"golemcloud/golem_sdk/schema" @schema`
and `"golemcloud/golem_sdk/interface/golem/core/types" @types` in `moon.pkg`.
The agent keeps the full `PromiseId` in its state, because the fingerprint cannot be
rebuilt from the agent metadata.
Call `start_approval` first and publish its returned oplog index and fingerprint together
with the known component and agent IDs to the approver before invoking `wait_for_approval`.
External completion requires agent-invoke permission. Async methods propagate
host errors; external callers must handle invocation failure rather than treating
it as approval. Lossy decoding replaces invalid UTF-8; validate the approval value
before acting on it.

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
