---
name: golem-wait-for-external-input-rust
description: "Waiting for external input using Golem promises in a Rust Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (Rust)

## Overview

A **Golem promise** lets an agent suspend its execution until an external event completes it. The agent creates a promise, passes the promise ID to an external system (another agent, a webhook, a UI, an HTTP API call), and then awaits the result. The Golem runtime durably suspends the agent — consuming no resources — until the promise is fulfilled.

## API

All functions are in the `golem_rust` crate:

| Function | Signature | Description |
|----------|-----------|-------------|
| `create_promise` | `fn create_promise() -> PromiseId` | Creates a new promise and returns its ID |
| `blocking_await_promise` | `fn blocking_await_promise(id: &PromiseId) -> Vec<u8>` | Blocks until the promise is completed (sync) |
| `await_promise` | `async fn await_promise(id: &PromiseId) -> Vec<u8>` | Awaits promise completion (async) |
| `complete_promise` | `fn complete_promise(id: &PromiseId, data: &[u8]) -> bool` | Completes a promise with raw bytes |

### JSON Helpers

For structured data, use the JSON wrappers from `golem_rust::json`:

| Function | Signature |
|----------|-----------|
| `blocking_await_promise_json<T: DeserializeOwned>` | `fn(id: &PromiseId) -> Result<T, serde_json::Error>` |
| `await_promise_json<T: DeserializeOwned>` | `async fn(id: &PromiseId) -> Result<T, serde_json::Error>` |
| `complete_promise_json<T: Serialize>` | `fn(id: &PromiseId, value: T) -> Result<bool, serde_json::Error>` |

## Imports

```rust
use golem_rust::{create_promise, complete_promise, blocking_await_promise, await_promise, PromiseId};
// For JSON helpers:
use golem_rust::json::{await_promise_json, blocking_await_promise_json, complete_promise_json};
```

## Usage Pattern

### 1. Create a Promise and Wait (Sync)

```rust
let promise_id = create_promise();
// Pass promise_id to an external system...
let data: Vec<u8> = blocking_await_promise(&promise_id);
```

### 2. Create a Promise and Wait (Async)

```rust
let promise_id = create_promise();
// Pass promise_id to an external system...
let data: Vec<u8> = await_promise(&promise_id).await;
```

### 3. Complete a Promise from Another Agent

```rust
complete_promise(&promise_id, b"done");
// Or with JSON:
complete_promise_json(&promise_id, MyResponse { status: "approved".into() }).unwrap();
```

## PromiseId Structure

A `PromiseId` contains an `agent_id` and an `oplog_idx`. To let an external system complete the promise via the Golem REST API, the agent must expose both fields. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json

{"oplogIdx": <oplog_idx>, "data": [<bytes>]}
```

## Full Example: Human-in-the-Loop Approval

```rust
use golem_rust::{
    agent_definition, agent_implementation, endpoint,
    create_promise, PromiseId,
};
use golem_rust::json::await_promise_json;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq)]
enum Decision {
    Approved,
    Rejected,
}

#[agent_definition(mount = "/workflows")]
pub trait WorkflowAgent {
    fn new(name: String) -> Self;

    async fn start_approval(&mut self) -> String;
}

struct WorkflowAgentImpl {
    name: String,
}

#[agent_implementation]
impl WorkflowAgent for WorkflowAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    #[endpoint(post = "/approve")]
    async fn start_approval(&mut self) -> String {
        // 1. Create a promise
        let promise_id = create_promise();

        // 2. Pass promise_id.oplog_idx to an external system (e.g. via RPC, HTTP, etc.)
        // The agent is now durably suspended.

        // 3. Wait for external completion
        let decision: Decision = await_promise_json(&promise_id)
            .await
            .expect("Invalid payload");

        if decision == Decision::Approved {
            format!("Workflow {} approved ✅", self.name)
        } else {
            format!("Workflow {} rejected ❌", self.name)
        }
    }
}
```

## Use Cases

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
