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

A `PromiseId` contains an `agent_id` and an `oplog_idx`. To let an external system complete the promise via the Golem REST API, the agent must expose both fields. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json
Authorization: Bearer <token-with-agent-invoke-permission>

{"oplogIdx": <oplog_idx>, "data": [<bytes>]}
```

## Full Example: Human-in-the-Loop Approval

```moonbit
#derive.agent
struct WorkflowAgent {
  name : String
  mut last_result : String
}

fn WorkflowAgent::new(name : String) -> WorkflowAgent {
  { name, last_result: "" }
}

/// Start an approval workflow that waits for external input
pub fn WorkflowAgent::start_approval(self : Self) -> UInt64 {
  let promise_id = @api.create_promise()
  // Return the oplog_idx so the external caller can complete the promise
  promise_id.oplog_idx
}

/// Wait for the approval to arrive and return the result
pub async fn WorkflowAgent::wait_for_approval(self : Self, oplog_idx : UInt64) -> String {
  let promise_id = @types.PromiseId::{
    agent_id: @api.get_self_metadata().agent_id,
    oplog_idx,
  }
  let data = @api.await_promise(promise_id)
  let result = @utf8.decode_lossy(data[:])
  self.last_result = result
  result
}
```

## Use Cases

Import `"moonbitlang/core/encoding/utf8" @utf8` in `moon.pkg`.
`get_self_metadata()` requires `self-view` in the effective permission card.
Call `start_approval` first and publish its returned index together with the known
component and agent IDs to the approver before invoking `wait_for_approval`.
External completion requires agent-invoke permission. Async methods propagate
host errors; external callers must handle invocation failure rather than treating
it as approval. Lossy decoding replaces invalid UTF-8; validate the approval value
before acting on it.

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
