---
name: golem-annotate-agent-kotlin
description: "Adding prompt and description annotations to Kotlin agent methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent methods for AI/LLM discovery."
---

# Annotating Agent Methods (Kotlin)

## Overview

Golem agents can annotate methods with `@Prompt` and `@Description` annotations. These provide metadata for AI/LLM tool discovery — agents with annotated methods can be used as tools by LLM-based systems.

## Annotations

- **`@Prompt("...")`** — A short instruction telling an LLM *when* to call this method
- **`@Description("...")`** — A longer explanation of what the method does, its parameters, and return value

## Usage

```kotlin
package inventory

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

@Agent(mount = "/inventory/{warehouseId}", description = "Manages stock levels for a warehouse")
class InventoryAgent(val warehouseId: String) : BaseAgent() {

    @Prompt("Look up the current stock level for a product")
    @Description("Returns the number of units in stock for the given product SKU. Returns 0 if the product is not found.")
    @Endpoint(get = "/stock/{sku}")
    fun checkStock(sku: String): Int = TODO()

    @Prompt("Add units of a product to inventory")
    @Description("Increases the stock count for the given SKU by the specified amount. Returns the new total.")
    @Endpoint(post = "/restock")
    fun restock(sku: String, quantity: Int): Int = TODO()

    @Prompt("Remove units of a product from inventory")
    @Description("Decreases the stock count for the given SKU. Throws if there is insufficient stock.")
    @Endpoint(post = "/pick")
    fun pick(sku: String, quantity: Int): Int = TODO()
}
```

## Guidelines

- `@Prompt` should be a natural-language instruction an LLM can match against a user request
- `@Description` should document behavior, edge cases, and expected inputs/outputs
- Both annotations are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
- `@Description` can also be placed on the class itself (alongside `@Agent`) to describe the agent type
