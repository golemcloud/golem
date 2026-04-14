---
name: golem-annotate-agent-ts
description: "Adding prompt and description annotations to TypeScript agents and their methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent classes or methods for AI/LLM discovery."
---

# Annotating Agents and Methods (TypeScript)

## Overview

Golem agents can annotate the agent class and its methods with `@prompt()` and `@description()` decorators. These provide metadata for AI/LLM tool discovery — agents with annotations can be used as tools by LLM-based systems.

## Annotations

- **`@prompt("...")`** — A short instruction telling an LLM *when* to call this method
- **`@description("...")`** — A longer explanation of what the agent or method does, its parameters, and return value

## Agent-Level Annotations

To describe the agent itself (its overall purpose), apply `@description()` as a **standalone class-level decorator** alongside `@agent()`. Do **NOT** pass `description` as a property inside the `@agent()` options — `AgentDecoratorOptions` does not accept it, and it will cause a TypeScript compilation error.

```typescript
// ✅ Correct — @description is a separate class decorator
@agent({ mount: "/api/v1/profiles" })
@description("Handles user profile management and preferences")
class ProfileAgent extends BaseAgent { ... }

// ❌ Wrong — description is not a valid @agent() option
@agent({ mount: "/api/v1/profiles", description: "..." })
class ProfileAgent extends BaseAgent { ... }
```

## Method-Level Annotations

```typescript
import { BaseAgent, agent, prompt, description } from '@golemcloud/golem-ts-sdk';

@agent({ mount: "/warehouses/{warehouseId}" })
@description("Manages product inventory for a warehouse")
class InventoryAgent extends BaseAgent {
    constructor(warehouseId: string) {
        super();
    }

    @prompt("Look up the current stock level for a product")
    @description("Returns the number of units in stock for the given product SKU. Returns 0 if the product is not found.")
    async checkStock(sku: string): Promise<number> {
        // ...
    }

    @prompt("Add units of a product to inventory")
    @description("Increases the stock count for the given SKU by the specified amount. Returns the new total.")
    async restock(sku: string, quantity: number): Promise<number> {
        // ...
    }

    @prompt("Remove units of a product from inventory")
    @description("Decreases the stock count for the given SKU. Throws if insufficient stock.")
    async pick(sku: string, quantity: number): Promise<number> {
        // ...
    }
}
```

## Guidelines

- `@description()` on the **class** describes the agent's overall purpose for LLM discovery
- `@prompt()` on a **method** should be a natural-language instruction an LLM can match against a user request
- `@description()` on a **method** should document behavior, edge cases, and expected inputs/outputs
- Both decorators are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
- Never pass `description` as a property of the `@agent()` decorator options — always use the standalone `@description()` decorator
