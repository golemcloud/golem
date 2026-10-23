---
name: golem-fire-and-forget-scala
description: "Triggering an agent invocation without waiting for the result in a Scala Golem project. Use when the user asks about fire-and-forget calls, async triggers, or enqueuing agent work."
---

# Fire-and-Forget Agent Invocation (Scala)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and returns immediately without waiting for the result. The target agent processes the invocation asynchronously.

## Usage

For stream-free methods, access `.trigger()` on the per-method wrapper. Durable guest clients return `Future[Unit]`; ephemeral guest clients return `Future[golem.runtime.rpc.InvocationReceipt]`. Compose or return that future to observe enqueue completion/errors, without waiting for the target's result. Streaming methods require awaited calls; bridge clients omit trigger helpers for those methods, while definition-derived clients can expose unsupported helpers that reject live stream inputs at runtime.

```scala
val counter = CounterAgentClient.get("my-counter")

// Fire-and-forget — returns immediately
counter.increment.trigger()

// With arguments
val processor = DataProcessorAgentClient.get("pipeline-1")
processor.processBatch.trigger(batchData)
```

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `.trigger` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```scala
// In AgentAImpl: return B's Future, keeping A's invocation open until B returns.
override def run(data: String): Future[String] =
  AgentBClient.get("b1").doWork(data)

// In AgentBImpl: enqueue the callback without waiting for A to execute it.
override def doWork(data: String): Future[String] = {
  val computed = s"processed: $data"
  AgentAClient.get("a1").onWorkDone.trigger(computed).map(_ => computed)
}
```

The traits must declare `run`, `doWork`, and `onWorkDone`; use an execution
context for `map`. B waits for callback enqueue acceptance, not callback execution.

## CLI Equivalent

From the command line, use `--trigger`:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```
