---
name: golem-configure-durability-ts
description: "Choosing between durable and ephemeral agents in a TypeScript Golem project. Use when the user asks about agent durability modes, making an agent stateless, or configuring agent persistence."
---

# Configuring Agent Durability (TypeScript)

## Durable Agents (Default)

By default, all Golem agents are **durable**:

- State persists across invocations, failures, and restarts
- Every side effect is recorded in an **oplog** (operation log)
- On failure, the agent is transparently recovered by replaying the oplog
- No special code needed — durability is automatic

A standard durable agent:

```typescript
import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }

    async getCount(): Promise<number> {
        return this.value;
    }
}
```

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is not needed:

- State is discarded after each invocation completes
- No oplog is maintained — lower overhead
- Useful for pure functions, request handlers, or adapters

```typescript
@agent({ mode: "ephemeral" })
class StatelessHandler extends BaseAgent {
    async handle(input: string): Promise<string> {
        return `processed: ${input}`;
    }
}
```

## When to Choose Which

| Use Case | Mode |
|----------|------|
| Counter, shopping cart, workflow orchestrator | **Durable** (default) |
| Stateless request processor, transformer | **Ephemeral** |
| Long-running saga or multi-step pipeline | **Durable** (default) |
| Pure computation, no side effects worth persisting | **Ephemeral** |
| Agent that calls external APIs with at-least-once semantics | **Durable** (default) |

When in doubt, use the default (durable). Ephemeral mode is an optimization for agents that genuinely don't need persistence.
