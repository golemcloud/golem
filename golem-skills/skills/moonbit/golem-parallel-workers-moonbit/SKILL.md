---
name: golem-parallel-workers-moonbit
description: "Fan out work to multiple parallel agents and collect results in a MoonBit Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning child agents for parallel work, forking, or aggregating results from multiple agents."
---

# Parallel Workers — Fan-Out / Fan-In (MoonBit)

## Overview

Golem agents process invocations **sequentially**. Async work within one invocation can overlap cooperatively, but this does not provide CPU-thread parallelism. Distribute independent CPU work across **multiple agent instances**. This skill covers two approaches:

1. **Child agents via `AgentClient::scoped`** — spawn separate agent instances, dispatch work, and collect results
2. **`@api.fork()`** — clone the current agent at the current execution point for lightweight parallel execution

## Approach 1: Child Agent Fan-Out

Spawn child agents, dispatch work via fire-and-forget triggers, and collect results via Golem promises.

Import `"moonbitlang/core/encoding/utf8" @utf8` in `moon.pkg`.
Raw `@types.PromiseId` has no `IntoWire` implementation. Transport its primitive
fields across RPC, then reconstruct the host record in the worker.

### Basic Pattern with Promises

```moonbit
#derive.agent
struct Coordinator {
  // no state needed
}

fn Coordinator::new() -> Coordinator { {  } }

/// Fan out work to child agents and collect results
pub async fn Coordinator::fan_out(self : Self, items : Array[String]) -> Array[String] {
  // Create one promise per child
  let promise_ids : Array[@types.PromiseId] = []
  for _ in items {
    promise_ids.push(@api.create_promise())
  }

  // Fire-and-forget: trigger each child with its promise ID
  for i, item in items {
    WorkerClient::scoped(i.to_uint64(), async fn(child) {
      let pid = promise_ids[i]
      let uuid = pid.agent_id.component_id.uuid
      child.trigger_process(item, uuid.high_bits, uuid.low_bits, pid.agent_id.agent_id, pid.oplog_idx)
    })
  }

  // Collect all results (agent suspends on each until completed)
  let results : Array[String] = []
  for pid in promise_ids {
    let bytes = @api.await_promise(pid)
    results.push(@utf8.decode_lossy(bytes[:]))
  }
  results
}
```

```moonbit
#derive.agent
struct Worker {
  id : UInt64
}

fn Worker::new(id : UInt64) -> Worker { { id, } }

/// Process data and complete the promise with the result
pub fn Worker::process(
  self : Self,
  data : String,
  component_high : UInt64,
  component_low : UInt64,
  agent_name : String,
  oplog_idx : UInt64,
) -> Unit {
  let promise_id = @types.PromiseId::{
    agent_id: {
      component_id: { uuid: { high_bits: component_high, low_bits: component_low } },
      agent_id: agent_name,
    },
    oplog_idx,
  }
  let result = "processed-\{data}"
  let payload = @utf8.encode(result[:])
  let _ = @api.complete_promise(promise_id, payload)
}
```

### Chunked Fan-Out

Batch children to limit concurrency:

```moonbit
pub async fn Coordinator::fan_out_chunked(
  self : Self,
  items : Array[String],
) -> Array[String] {
  let results : Array[String] = []
  let chunk_size = 5
  let mut offset = 0

  while offset < items.length() {
    let end = @math.minimum(offset + chunk_size, items.length())
    let promise_ids : Array[@types.PromiseId] = []
    for i = offset; i < end; i = i + 1 {
      let pid = @api.create_promise()
      promise_ids.push(pid)
      WorkerClient::scoped(i.to_uint64(), async fn(child) {
        let uuid = pid.agent_id.component_id.uuid
        child.trigger_process(items[i], uuid.high_bits, uuid.low_bits, pid.agent_id.agent_id, pid.oplog_idx)
      })
    }
    for pid in promise_ids {
      let bytes = @api.await_promise(pid)
      results.push(@utf8.decode_lossy(bytes[:]))
    }
    offset = end
  }
  results
}
```

## Approach 2: `@api.fork()`

`@api.fork()` clones the current agent at the current execution point, creating a new agent instance with the same state but a unique phantom ID. Only a durable agent can fork, and the forked agent is durable too. Use Golem promises to synchronize between the original and forked agents.

### Basic Fork Pattern

```moonbit
#derive.agent
struct ForkAgent {
  mut result : String
}

fn ForkAgent::new() -> ForkAgent { { result: "" } }

/// Fork the agent and collect result via a promise
pub async fn ForkAgent::parallel_compute(self : Self) -> String {
  let promise_id = @api.create_promise()

  match @api.fork() {
    Original(_details) => {
      // Wait for the forked agent to complete the promise
      let bytes = @api.await_promise(promise_id)
      let forked_result = @utf8.decode_lossy(bytes[:])
      "Combined: original + \{forked_result}"
    }
    Forked(_details) => {
      // Do work in the forked copy
      let computed = "forked-result"
      let payload = @utf8.encode(computed[:])
      let _ = @api.complete_promise(promise_id, payload)
      "forked done" // Only seen by the forked agent
    }
  }
}
```

### Multi-Fork Fan-Out

Fork multiple times for N-way parallelism:

```moonbit
pub async fn ForkAgent::multi_fork(self : Self, n : UInt64) -> Array[String] {
  let promise_ids : Array[@types.PromiseId] = []
  for _ = 0L; _ < n.to_int64(); _ = _ + 1L {
    promise_ids.push(@api.create_promise())
  }

  for i = 0; i < promise_ids.length(); i = i + 1 {
    match @api.fork() {
      Original(_) => {
        // Continue to next fork
      }
      Forked(_) => {
        // Each forked agent does its slice of work
        let output = "result-from-fork-\{i}"
        let payload = @utf8.encode(output[:])
        let _ = @api.complete_promise(promise_ids[i], payload)
        return [] // Forked agent exits here
      }
    }
  }

  // Original agent collects all results
  let results : Array[String] = []
  for pid in promise_ids {
    let bytes = @api.await_promise(pid)
    results.push(@utf8.decode_lossy(bytes[:]))
  }
  results
}
```

## When to Use Which Approach

| Criteria | Child Agents | `@api.fork()` |
|----------|-------------|---------------|
| Work is **independent** and stateless | ✅ Best fit | Works but overkill |
| Need to **share current state** with workers | ❌ Must pass via args | ✅ Forked copy inherits state |
| Workers need **persistent identity** | ✅ Each has own ID | ❌ Each forked agent gets a generated phantom ID |
| Number of parallel tasks is **dynamic** | ✅ Spawn as many as needed | ✅ Fork in a loop |
| Need **simple error isolation** | ✅ Child failure doesn't crash parent | ⚠️ Forked agent shares oplog lineage |

## Key Points

- **No threads**: Golem is single-threaded per agent — parallelism is achieved by distributing across agent instances
- **Durability**: All RPC calls, promises, and fork operations are durably recorded — work survives crashes
- **Deadlock avoidance**: Never have two agents awaiting each other synchronously — use `trigger_` to break cycles
- **Cleanup**: Child agents persist after the coordinator finishes; delete them explicitly if they hold unwanted state
- **Scoped clients**: Always prefer `AgentClient::scoped` over manual `get`/`drop` for client lifecycle management
- **Aggregation**: Collect results by iterating over promise IDs and calling `@api.await_promise` for each
