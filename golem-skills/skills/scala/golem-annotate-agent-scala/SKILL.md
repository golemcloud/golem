---
name: golem-annotate-agent-scala
description: "Adding prompt and description annotations to Scala agent methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent methods for AI/LLM discovery."
---

# Annotating Agent Methods (Scala)

## Overview

Golem agents can annotate methods with `@prompt` and `@description` annotations. These provide metadata for AI/LLM tool discovery — agents with annotated methods can be used as tools by LLM-based systems.

## Annotations

- **`@prompt("...")`** — A short instruction telling an LLM *when* to call this method
- **`@description("...")`** — A longer explanation of what the method does, its parameters, and return value

## Usage

```scala
import golem.runtime.annotations.{agentDefinition, description, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/inventory/{warehouseId}")
trait InventoryAgent extends BaseAgent {
  class Id(val warehouseId: String)

  @prompt("Look up the current stock level for a product")
  @description("Returns the number of units in stock for the given product SKU. Returns 0 if the product is not found.")
  def checkStock(sku: String): Future[Int]

  @prompt("Add units of a product to inventory")
  @description("Increases the stock count for the given SKU by the specified amount. Returns the new total.")
  def restock(sku: String, quantity: Int): Future[Int]

  @prompt("Remove units of a product from inventory")
  @description("Decreases the stock count for the given SKU. Returns a Left if insufficient stock.")
  def pick(sku: String, quantity: Int): Future[Either[String, Int]]
}
```

## Guidelines

- `@prompt` should be a natural-language instruction an LLM can match against a user request
- `@description` should document behavior, edge cases, and expected inputs/outputs
- Both annotations are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
