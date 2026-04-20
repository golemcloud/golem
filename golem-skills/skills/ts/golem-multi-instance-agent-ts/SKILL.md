---
name: golem-multi-instance-agent-ts
description: "Using phantom agents in TypeScript to create multiple agent instances with the same constructor parameters. Use when the user needs multiple distinct agents sharing constructor values, or asks about phantom agents, phantom IDs, getPhantom/newPhantom, or multi-instance agents in TypeScript."
---

# Phantom Agents in TypeScript

Phantom agents allow creating **multiple distinct agent instances** that share the same constructor parameters. Normally, an agent is uniquely identified by its constructor parameter values — calling `get` with the same parameters always returns the same agent. Phantom agents add an extra **phantom ID** (a UUID) to the identity, so you can have many independent instances with identical parameters.

## Agent ID Format

A phantom agent's ID appends the phantom UUID in square brackets:

```
agent-type(param1, param2)[a09f61a8-677a-40ea-9ebe-437a0df51749]
```

A non-phantom agent ID has no bracket suffix:

```
agent-type(param1, param2)
```

## Creating and Addressing Phantom Agents (RPC)

The `@agent()` decorator generates static methods on the agent class:

| Method | Description |
|--------|-------------|
| `AgentClass.get(params...)` | Get or create a **non-phantom** agent identified solely by its parameters |
| `AgentClass.newPhantom(params...)` | Create a **new phantom** agent with a freshly generated random UUID |
| `AgentClass.getPhantom(uuid, params...)` | Get or create a phantom agent with a **specific** UUID |

### Example

```typescript
import { BaseAgent, agent, Uuid } from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
    private count: number = 0;

    constructor(private readonly name: string) {
        super();
    }

    async increment(): Promise<number> {
        this.count += 1;
        return this.count;
    }
}

// --- In another agent, using the generated static methods: ---

// Non-phantom: always the same agent for the same name
const counter = CounterAgent.get("shared");

// New phantom: creates a brand new independent instance
const phantom1 = CounterAgent.newPhantom("shared");
const phantom2 = CounterAgent.newPhantom("shared");
// phantom1 and phantom2 are different agents, both with name="shared"

// Reconnect to an existing phantom by its UUID
const existingId: Uuid = /* previously saved UUID */;
const samePhantom = CounterAgent.getPhantom(existingId, "shared");
```

### WithConfig Variants

If the agent has `@config` fields, additional methods are generated:

- `AgentClass.getWithConfig(params..., configFields...)`
- `AgentClass.newPhantomWithConfig(params..., configFields...)`
- `AgentClass.getPhantomWithConfig(uuid, params..., configFields...)`

## Querying the Phantom ID from Inside an Agent

An agent can check its own phantom ID using the `phantomId()` method inherited from `BaseAgent`:

```typescript
@agent()
class MyAgent extends BaseAgent {
    constructor() { super(); }

    async whoAmI(): Promise<string> {
        const phantom = this.phantomId(); // Uuid | undefined
        if (phantom) {
            return `I am a phantom agent with ID: ${phantom.toString()}`;
        }
        return "I am a regular agent";
    }
}
```

## HTTP-Mounted Phantom Agents

When an agent is mounted as an HTTP endpoint, you can set `phantom: true` in the decorator options to make every incoming HTTP request create a **new phantom instance** automatically:

```typescript
@agent({ mount: "/api", phantom: true })
class RequestHandler extends BaseAgent {
    async handle(input: string): Promise<string> {
        return `processed: ${input}`;
    }
}
```

Each HTTP request will be handled by a fresh agent instance with its own phantom ID, even though all instances share the same constructor parameters.

## Key Points

- Phantom agents are **fully durable** — they persist just like regular agents.
- The phantom ID is a standard UUID (v4 by default when using `newPhantom`).
- `newPhantom` generates the UUID internally via `Uuid.generate()`.
- `getPhantom` is idempotent: calling it with the same UUID and parameters always returns the same agent.
- Phantom and non-phantom agents with the same constructor parameters are **different agents** — they do not share state.
