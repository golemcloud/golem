---
name: golem-annotate-agent-ts
description: "Adding prompt and description annotations to TypeScript agent methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent methods for AI/LLM discovery."
---

# Annotating Agent Methods (TypeScript)

## Overview

Golem agents can annotate methods with `@prompt()` and `@description()` decorators. These provide metadata for AI/LLM tool discovery — agents with annotated methods can be used as tools by LLM-based systems.

## Annotations

- **`@prompt("...")`** — A short instruction telling an LLM *when* to call this method
- **`@description("...")`** — A longer explanation of what the method does, its parameters, and return value

## Usage

```typescript
import { BaseAgent, agent, prompt, description } from '@golemcloud/golem-ts-sdk';

@agent()
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

- `@prompt()` should be a natural-language instruction an LLM can match against a user request
- `@description()` should document behavior, edge cases, and expected inputs/outputs
- Both decorators are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
