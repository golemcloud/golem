---
name: golem-fire-and-forget-moonbit
description: "Triggering a MoonBit Golem agent invocation without waiting for the result. Use when the user asks to fire-and-forget, trigger asynchronously, or enqueue an agent call from within agent code."
---

# Fire-and-Forget Agent Invocation (MoonBit)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and returns immediately without waiting for the result. The target agent processes the invocation asynchronously. This is for **agent-to-agent calls from code**, not from the CLI.

For CLI fire-and-forget, see the `golem-trigger-agent-moonbit` skill.

## Usage

Methods with recursively stream-free inputs and outputs have an auto-generated `trigger_` variant. Run these fragments inside an async function: `AgentClient::scoped` and its callback are async, while the `trigger_` enqueue itself is synchronous. Propagate async to helpers and callers:

```moonbit
AgentClient::scoped("my-counter", async fn(client) {
  client.trigger_increment()
})
```

With arguments:

```moonbit
DataProcessorClient::scoped("pipeline-1", async fn(client) {
  client.trigger_process_batch(batch_data)
})
```

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `trigger_` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```moonbit
// In AgentA — calls AgentB and waits
AgentBClient::scoped("b1", async fn(client) {
  let result = client.do_work(data) // OK: awaited call
  // ...
})

// In AgentB — notifies AgentA without waiting (would deadlock if awaited)
AgentAClient::scoped("a1", async fn(client) {
  client.trigger_on_work_done(result) // OK: fire-and-forget
})
```

## Example: Fan-Out

```moonbit
let regions : Array[String] = ["us-east", "us-west", "eu-central"]
for region in regions {
  RegionProcessorClient::scoped(region, async fn(client) {
    client.trigger_run_report(report_id)
  })
}
```

## CLI Equivalent

From the command line, use `--trigger`:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```

See the `golem-trigger-agent-moonbit` skill for full CLI details.
