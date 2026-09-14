---
name: golem-annotate-agent-moonbit
description: "Adding prompt and description annotations to MoonBit agent methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent methods for AI/LLM discovery."
---

# Annotating Agent Methods (MoonBit)

## Overview

Golem agents can annotate methods with doc comments (`///`) for descriptions and `#derive.prompt_hint("...")` for prompt hints. These provide metadata for AI/LLM tool discovery — agents with annotated methods can be used as tools by LLM-based systems.

## Annotations

- **`#derive.prompt_hint("...")`** — A short instruction telling an LLM *when* to call this method; placed before the method
- **`///` doc comments** — A longer explanation of what the method does, its parameters, and return value

## Usage

```moonbit
#derive.agent
struct InventoryAgent {
  warehouse_id: String
  // ...
}

fn InventoryAgent::new(warehouse_id: String) -> InventoryAgent {
  { warehouse_id }
}

/// Returns the number of units in stock for the given product SKU.
/// Returns 0 if the product is not found.
#derive.prompt_hint("Look up the current stock level for a product")
pub fn InventoryAgent::check_stock(self: Self, sku: String) -> UInt {
  // ...
  0
}

/// Increases the stock count for the given SKU by the specified amount.
/// Returns the new total.
#derive.prompt_hint("Add units of a product to inventory")
pub fn InventoryAgent::restock(self: Self, sku: String, quantity: UInt) -> UInt {
  // ...
  0
}

/// Decreases the stock count for the given SKU.
/// Returns an error if insufficient stock.
#derive.prompt_hint("Remove units of a product from inventory")
pub fn InventoryAgent::pick(self: Self, sku: String, quantity: UInt) -> Result[UInt, String] {
  // ...
  Ok(0)
}
```

## Guidelines

- `#derive.prompt_hint` should be a natural-language instruction an LLM can match against a user request
- `///` doc comments should document behavior, edge cases, and expected inputs/outputs
- Both annotations are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
