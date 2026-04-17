---
name: golem-add-agent-ts
description: "Adding a new TypeScript agent to a Golem component. Use when the user asks to create, add, or define a new agent type, implement an agent class, or add agent methods in a TypeScript Golem project."
---

# Adding a New Agent to a TypeScript Golem Component

## Overview

An **agent** is a durable, stateful unit of computation in Golem. Each agent type is a class decorated with `@agent()` that extends `BaseAgent` from `@golemcloud/golem-ts-sdk`.

## Steps

1. **Create the agent file** — add a new file `src/<agent-name>.ts`
2. **Define the agent class** — decorate with `@agent()`, extend `BaseAgent`
3. **Import from `main.ts`** — add `import './<agent-name>';` to `src/main.ts`
4. **Build** — run `golem build` to verify

`src/main.ts` is the entrypoint module that must import each agent module for side effects. Agent classes do not need to be exported for discovery — importing the module is sufficient because `@agent()` registers the class.

## Agent Definition

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

## Custom Types

Use TypeScript type aliases or interfaces for parameters and return types. Use **named types** instead of anonymous inline object types for better interoperability. **TypeScript enums are not supported** — use string literal unions instead:

```typescript
type Coordinates = { lat: number; lon: number };
type WeatherReport = { temperature: number; description: string };
type Priority = "low" | "medium" | "high";

@agent()
class WeatherAgent extends BaseAgent {
    constructor(apiKey: string) {
        super();
    }

    async getWeather(coords: Coordinates): Promise<WeatherReport> {
        // ...
    }
}
```

## Related Skills

- Load `golem-js-runtime` for details on the QuickJS runtime environment, available Web/Node.js APIs, and npm compatibility
- Load `golem-file-io-ts` for reading and writing files from agent code

## Key Constraints

- All agent classes must extend `BaseAgent` and be decorated with `@agent()`
- Constructor parameters define agent identity — they must be serializable types
- TypeScript **enums are not supported** — use string literal unions instead
- Agents are created implicitly on first invocation — no separate creation step
- Invocations are processed sequentially in a single thread — no concurrency within a single agent
- The build pipeline uses `golem-typegen` for type metadata extraction; ensure `experimentalDecorators` and `emitDecoratorMetadata` are enabled in `tsconfig.json`
