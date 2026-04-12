---
name: golem-atomic-block-scala
description: "Using atomic blocks, persistence control, and oplog management in a Scala Golem project. Use when the user asks about atomic operations, persistence levels, oplog commit, or the Host API."
---

# Atomic Blocks and Durability Controls (Scala)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity or atomicity.

## Host API — Mark Begin/End Operation

Group operations into atomic blocks using the Host API:

```scala
import golem.HostApi

val begin = HostApi.markBeginOperation()
// ... do work — all operations in this block are atomic ...
HostApi.markEndOperation(begin)
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

## Use Cases

- **Exactly-once external calls**: Wrap a payment API call in an atomic block with an idempotency key
- **Performance optimization**: Disable persistence for idempotent computation-heavy sections
- **Consistency guarantees**: Ensure the oplog is replicated before acknowledging a critical operation
