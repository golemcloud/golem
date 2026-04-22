---
name: golem-wait-for-external-input-scala
description: "Waiting for external input using Golem promises in a Scala Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (Scala)

## Overview

A **Golem promise** lets an agent suspend its execution until an external event completes it. The agent creates a promise, passes the promise ID to an external system (another agent, a webhook, a UI, an HTTP API call), and then awaits the result. The Golem runtime durably suspends the agent — consuming no resources — until the promise is fulfilled.

## API

All functions are on the `golem.HostApi` object:

| Method | Signature | Description |
|--------|-----------|-------------|
| `createPromise` | `(): PromiseId` | Creates a new promise and returns its ID |
| `awaitPromiseBlocking` | `(id: PromiseId): Array[Byte]` | Blocks until the promise is completed (sync) |
| `awaitPromise` | `(id: PromiseId): Future[Array[Byte]]` | Awaits promise completion (async Future) |
| `completePromise` | `(id: PromiseId, data: Array[Byte]): Boolean` | Completes a promise with raw bytes |

### JSON Helpers (Schema-based)

For structured data, use the JSON variants which require an implicit `zio.blocks.schema.Schema[A]`:

| Method | Signature |
|--------|-----------|
| `awaitPromiseBlockingJson[A]` | `(id: PromiseId)(implicit Schema[A]): A` |
| `awaitPromiseJson[A]` | `(id: PromiseId)(implicit Schema[A]): Future[A]` |
| `completePromiseJson[A]` | `(id: PromiseId, value: A)(implicit Schema[A]): Boolean` |

## Imports

```scala
import golem.HostApi
import golem.HostApi.PromiseId
```

## Usage Pattern

### 1. Create a Promise and Wait (Blocking)

```scala
val promiseId = HostApi.createPromise()
// Pass promiseId to an external system...

// Agent is durably suspended here until the promise is completed
val data: Array[Byte] = HostApi.awaitPromiseBlocking(promiseId)
```

### 2. Create a Promise and Wait with JSON Decoding

```scala
case class Decision(status: String) derives Schema

val promiseId = HostApi.createPromise()
val decision: Decision = HostApi.awaitPromiseBlockingJson[Decision](promiseId)
```

### 3. Complete a Promise from Another Agent

```scala
HostApi.completePromise(promiseId, "approved".getBytes)
// Or with JSON:
HostApi.completePromiseJson(promiseId, Decision("approved"))
```

## PromiseId Structure

A `PromiseId` contains an `agentId` and an `oplogIdx`. To let an external system complete the promise via the Golem REST API, the agent must expose both fields. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json

{"oplogIdx": <oplog_idx>, "data": [<bytes>]}
```

## Full Example: Human-in-the-Loop Approval

```scala
import golem.*
import zio.blocks.schema.Schema

case class Decision(status: String) derives Schema

@agentDefinition
trait WorkflowAgent extends BaseAgent {
  class Id(name: String)

  def startApproval(): String
}

@agentImplementation()
class WorkflowAgentImpl extends WorkflowAgent {
  private var name: String = ""

  override def init(id: Id): Unit = {
    name = id.name
  }

  override def startApproval(): String = {
    // 1. Create a promise
    val promiseId = HostApi.createPromise()

    // 2. Pass promiseId.oplogIdx to an external system
    // The agent is now durably suspended.

    // 3. Wait for external completion
    val decision = HostApi.awaitPromiseBlockingJson[Decision](promiseId)

    if (decision.status == "approved")
      s"Workflow $name approved ✅"
    else
      s"Workflow $name rejected ❌"
  }
}
```

## Use Cases

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
