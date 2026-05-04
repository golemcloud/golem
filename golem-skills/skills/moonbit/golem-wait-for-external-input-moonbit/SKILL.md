---
name: golem-wait-for-external-input-moonbit
description: "Waiting for external input using Golem promises in a MoonBit Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (MoonBit)

## Overview

A **Golem promise** lets an agent suspend its execution until an external event completes it. The agent creates a promise, passes the promise ID to an external system (another agent, a webhook, a UI, an HTTP API call), and then awaits the result. The Golem runtime durably suspends the agent — consuming no resources — until the promise is fulfilled.

## API

All functions are in the `@api` package of the Golem MoonBit SDK:

| Function | Signature | Description |
|----------|-----------|-------------|
| `@api.create_promise` | `() -> PromiseId` | Creates a new promise and returns its ID |
| `@api.await_promise` | `(PromiseId) -> Bytes` | Blocks until the promise is completed |
| `@api.complete_promise` | `(PromiseId, Bytes) -> Bool` | Completes a promise with a byte payload |
| `@api.get_promise` | `(PromiseId) -> PromiseResult` | Gets a handle for polling/getting the result |

## Usage Pattern

### 1. Create a Promise and Wait

```moonbit
let promise_id = @api.create_promise()
// Pass promise_id to an external system...

// Agent is durably suspended here until the promise is completed
let data : Bytes = @api.await_promise(promise_id)
```

### 2. Complete a Promise from Another Agent

```moonbit
let payload = "approved"
@api.complete_promise(promise_id, Bytes::from_array(payload.to_array().map(fn(c) { c.to_int().to_byte() })))
```

### 3. Advanced: Poll without Blocking

```moonbit
let promise_result = @api.get_promise(promise_id)
let pollable = promise_result.subscribe()
// Use pollable with poll infrastructure
match promise_result.get() {
  Some(data) => // promise completed
  None => // not ready yet
}
```

## PromiseId Structure

A `PromiseId` contains an `agent_id` and an `oplog_idx`. To let an external system complete the promise via the Golem REST API, the agent must expose both fields. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json

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
pub fn WorkflowAgent::wait_for_approval(self : Self, oplog_idx : UInt64) -> String {
  let promise_id = @types.PromiseId::{
    agent_id: @api.get_self_agent_id(),
    oplog_idx,
  }
  let data = @api.await_promise(promise_id)
  let result = String::from_array(data.to_array().map(fn(b) { Char::from_int(b.to_int()) }))
  self.last_result = result
  result
}
```

## Use Cases

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
