---
name: golem-wait-for-external-input-rust
description: "Waiting for external input using Golem promises in a Rust Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (Rust)

## Overview

A **Golem promise** lets a durable agent wait for an external event. Creating a promise does not
suspend execution. Publish its ID before awaiting it. An incomplete wait can durably suspend after
the runtime's suspension delay; this is not an immediate zero-resource guarantee. Completion is
first-write-wins: subsequent completions do not replace the original bytes.

## API

All functions are in the `golem_rust` crate:

| Function | Signature | Description |
|----------|-----------|-------------|
| `create_promise` | `fn create_promise() -> PromiseId` | Creates a new promise and returns its ID |
| `blocking_await_promise` | `fn blocking_await_promise(id: &PromiseId) -> Vec<u8>` | Blocks until the promise is completed (sync) |
| `await_promise` | `async fn await_promise(id: &PromiseId) -> Vec<u8>` | Awaits promise completion (async) |
| `complete_promise` | `fn complete_promise(id: &PromiseId, data: &[u8]) -> bool` | Completes a promise with raw bytes |

### JSON Helpers

For structured data, use the JSON wrappers reexported from `golem_rust` (with its default `json` feature):

| Function | Signature |
|----------|-----------|
| `blocking_await_promise_json<T: DeserializeOwned>` | `fn(id: &PromiseId) -> Result<T, serde_json::Error>` |
| `await_promise_json<T: DeserializeOwned>` | `async fn(id: &PromiseId) -> Result<T, serde_json::Error>` |
| `complete_promise_json<T: Serialize>` | `fn(id: &PromiseId, value: T) -> Result<bool, serde_json::Error>` |

## Imports

```rust
use golem_rust::{create_promise, complete_promise, blocking_await_promise, await_promise, PromiseId};
// For JSON helpers:
use golem_rust::{await_promise_json, blocking_await_promise_json, complete_promise_json};
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
Authorization: Bearer <GOLEM_TOKEN>
Content-Type: application/json

{"oplogIdx": <oplog_idx>, "data": [<bytes>]}
```

## Full Example: Human-in-the-Loop Approval

```rust
use golem_rust::{
    agent_definition, agent_implementation, endpoint,
    create_promise, PromiseId,
};
use golem_rust::await_promise_json;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq)]
enum Decision {
    Approved,
    Rejected,
}

#[agent_definition(mount = "/workflows/{name}")]
pub trait WorkflowAgent {
    fn new(name: String) -> Self;

    #[endpoint(post = "/approve")]
    async fn start_approval(&mut self) -> Result<String, String>;
}

struct WorkflowAgentImpl {
    name: String,
}

#[agent_implementation]
impl WorkflowAgent for WorkflowAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn start_approval(&mut self) -> Result<String, String> {
        // 1. Create a promise
        let promise_id = create_promise();

        // Publish the full ID to an approval service before waiting.
        // That service maps agent_id to the component/worker REST path.
        let request = golem_rust::http::Request::post("https://approvals.example.com/register")
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&promise_id).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let response = golem_rust::http::send(request).await.map_err(|e| e.to_string())?;
        let status = response.status();
        let _ = golem_rust::http::response_bytes(response).await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("Approval registration failed: {status}"));
        }

        // 3. Wait for external completion
        let decision: Decision = await_promise_json(&promise_id)
            .await
            .map_err(|e| format!("Invalid approval payload: {e}"))?;

        if decision == Decision::Approved {
            Ok(format!("Workflow {} approved ✅", self.name))
        } else {
            Ok(format!("Workflow {} rejected ❌", self.name))
        }
    }
}
```

## Use Cases

The external service completes the promise through the authenticated REST API, for example:

```shell
curl -X POST "$GOLEM_BASE_URL/v1/components/$COMPONENT_ID/workers/$AGENT_NAME_ENCODED/complete" \
  -H "Authorization: Bearer $GOLEM_TOKEN" \
  -H 'Content-Type: application/json' \
  --data "{\"oplogIdx\":$OPLOG_IDX,\"data\":[34,65,112,112,114,111,118,101,100,34]}"
```

The bytes encode the JSON string `"Approved"`. URL-encode the agent name in the path. Malformed
JSON returns a domain error in the example; a later completion cannot repair the first payload.

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
