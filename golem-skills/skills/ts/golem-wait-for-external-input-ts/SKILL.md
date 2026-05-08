---
name: golem-wait-for-external-input-ts
description: "Waiting for external input using Golem promises in a TypeScript Golem project. Use when the user asks about promises, waiting for external events, human-in-the-loop workflows, pausing an agent until an external signal, or suspending execution until data arrives from outside."
---

# Waiting for External Input with Golem Promises (TypeScript)

## Overview

A **Golem promise** lets an agent suspend its execution until an external event completes it. The agent creates a promise, passes the promise ID to an external system (another agent, a webhook, a UI, an HTTP API call), and then awaits the result. The Golem runtime durably suspends the agent — consuming no resources — until the promise is fulfilled.

## API

All functions are in `@golemcloud/golem-ts-sdk`:

| Function | Signature | Description |
|----------|-----------|-------------|
| `createPromise` | `() => PromiseId` | Creates a new promise and returns its ID |
| `awaitPromise` | `(id: PromiseId) => Promise<Uint8Array>` | Awaits promise completion (non-blocking) |
| `completePromise` | `(id: PromiseId, data: Uint8Array) => boolean` | Completes a promise with raw bytes |

## Imports

```typescript
import { createPromise, awaitPromise, completePromise } from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from '@golemcloud/golem-ts-sdk';
```

## Usage Pattern

### 1. Create a Promise and Wait

```typescript
const promiseId = createPromise();
// Pass promiseId to an external system...

// Agent is durably suspended here until the promise is completed
const resultBytes = await awaitPromise(promiseId);
const result = new TextDecoder().decode(resultBytes);
```

### 2. Complete a Promise from Another Agent

```typescript
const data = new TextEncoder().encode(JSON.stringify({ status: "approved" }));
completePromise(promiseId, data);
```

### 3. Decode JSON from a Completed Promise

```typescript
const promiseId = createPromise();
const bytes = await awaitPromise(promiseId);
const decision = JSON.parse(new TextDecoder().decode(bytes));
```

## PromiseId Structure

A `PromiseId` contains an `agentId` and an `oplogIdx`. To let an external system complete the promise via the Golem REST API, the agent must expose both fields. The external caller then sends:

```
POST /v1/components/{component_id}/workers/{agent_name}/complete
Content-Type: application/json

{"oplogIdx": <oplog_idx>, "data": [<bytes>]}
```

## Full Example: Human-in-the-Loop Approval

```typescript
import { BaseAgent, agent, endpoint } from '@golemcloud/golem-ts-sdk';
import { createPromise, awaitPromise, completePromise } from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from '@golemcloud/golem-ts-sdk';

@agent({ mount: "/workflows/{name}" })
class WorkflowAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super();
        this.name = name;
    }

    @endpoint({ post: "/approve" })
    async startApproval(): Promise<string> {
        // 1. Create a promise
        const promiseId = createPromise();

        // 2. Pass promiseId.oplogIdx to an external system
        // The agent is now durably suspended.

        // 3. Wait for external completion
        const bytes = await awaitPromise(promiseId);
        const decision = JSON.parse(new TextDecoder().decode(bytes));

        if (decision.status === "approved") {
            return `Workflow ${this.name} approved ✅`;
        } else {
            return `Workflow ${this.name} rejected ❌`;
        }
    }
}
```

## Use Cases

- **Human-in-the-loop**: Pause a workflow until a human approves or rejects
- **Webhook callbacks**: Wait for an external HTTP callback to arrive
- **Inter-agent synchronization**: One agent creates a promise, another completes it
- **External event ingestion**: Suspend until an IoT sensor, payment gateway, or third-party API sends a signal
