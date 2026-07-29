---
name: golem-annotate-agent-rust
description: "Adding prompt and description annotations to Rust agent methods. Use when the user asks to add descriptions, prompts, or documentation metadata to agent methods for AI/LLM discovery."
---

# Annotating Agent Methods (Rust)

## Overview

Golem agents can annotate methods with `#[prompt]` and `#[description]` attributes. These provide metadata for AI/LLM tool discovery — agents with annotated methods can be used as tools by LLM-based systems.

## Annotations

- **`#[prompt("...")]`** — A short instruction telling an LLM *when* to call this method
- **`#[description("...")]`** — A longer explanation of what the method does, its parameters, and return value

## Usage

```rust
use golem_rust::{agent_definition, agent_implementation, prompt, description};

#[agent_definition]
pub trait InventoryAgent {
    fn new(warehouse_id: String) -> Self;

    #[prompt("Look up the current stock level for a product")]
    #[description("Returns the number of units in stock for the given product SKU. Returns 0 if the product is not found.")]
    fn check_stock(&self, sku: String) -> u32;

    #[prompt("Add units of a product to inventory")]
    #[description("Increases the stock count for the given SKU by the specified amount. Returns the new total.")]
    fn restock(&mut self, sku: String, quantity: u32) -> u32;

    #[prompt("Remove units of a product from inventory")]
    #[description("Decreases the stock count for the given SKU. Returns err if insufficient stock.")]
    fn pick(&mut self, sku: String, quantity: u32) -> Result<u32, String>;
}
```

## Guidelines

- `#[prompt]` should be a natural-language instruction an LLM can match against a user request
- `#[description]` should document behavior, edge cases, and expected inputs/outputs
- Both annotations are optional — omit them for internal methods not intended for LLM discovery
- Annotations have no effect on runtime behavior; they are purely metadata
