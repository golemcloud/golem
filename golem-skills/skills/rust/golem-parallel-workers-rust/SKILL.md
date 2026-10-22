---
name: golem-parallel-workers-rust
description: "Fan out work to multiple parallel agents and collect results in a Rust Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning child agents for parallel work, forking, or aggregating results from multiple agents."
---

# Parallel Workers — Fan-Out / Fan-In (Rust)

## Overview

Golem agents process invocations **sequentially**. Within one invocation, async I/O and stream tasks can overlap cooperatively; this is not CPU-thread parallelism. Distribute work across **multiple agent instances** for independent invocation execution. This skill covers two approaches:

1. **Child agents via `AgentClient::get(id)`** — spawn separate agent instances, dispatch work, and collect results
2. **`fork()`** — clone the current agent at the current execution point for lightweight parallel execution

Use the `futures_concurrency` crate for structured concurrency when aggregating results from multiple async operations.

## Prerequisites

Add `futures-concurrency` to your component's `Cargo.toml`:

```toml
[dependencies]
futures-concurrency = "7"
```

## Approach 1: Child Agent Fan-Out

Spawn child agents, dispatch work concurrently, and collect results with `Join` or `TryJoin`.

### Basic Pattern with `Join`

```rust
use futures_concurrency::prelude::*;
use golem_rust::{agent_definition, agent_implementation, await_promise};

#[agent_definition]
pub trait Coordinator {
    fn new() -> Self;
    async fn fan_out(&mut self, items: Vec<String>) -> Vec<String>;
}

struct CoordinatorImpl;

#[agent_implementation]
impl Coordinator for CoordinatorImpl {
    fn new() -> Self { Self }

    async fn fan_out(&mut self, items: Vec<String>) -> Vec<String> {
        // Build a Vec of futures — one per child agent
        let futures: Vec<_> = items.iter().enumerate().map(|(i, item)| {
            let child = WorkerClient::get(i as u64);
            let item = item.clone();
            async move { child.process(item).await }
        }).collect();

        // Await all concurrently using futures_concurrency::Join
        futures.join().await
    }
}
```

### Fire-and-Forget with Promise Collection

For long-running work, trigger children with fire-and-forget and collect results via Golem promises:

```rust
use futures_concurrency::prelude::*;
use golem_rust::{
    agent_definition, agent_implementation,
    create_promise, await_promise, complete_promise, PromiseId,
};
use golem_rust::{await_promise_json, complete_promise_json};
use serde::{Deserialize, Serialize};

#[agent_definition]
pub trait Coordinator {
    fn new() -> Self;
    async fn dispatch_and_collect(&mut self, regions: Vec<String>) -> Vec<String>;
}

struct CoordinatorImpl;

#[agent_implementation]
impl Coordinator for CoordinatorImpl {
    fn new() -> Self { Self }

    async fn dispatch_and_collect(&mut self, regions: Vec<String>) -> Vec<String> {
        // Create one promise per child
        let promise_ids: Vec<PromiseId> = regions.iter().map(|_| create_promise()).collect();

        // Fire-and-forget: trigger each child with its promise ID
        for (region, pid) in regions.iter().zip(&promise_ids) {
            let mut child = RegionWorkerClient::get(region.clone());
            child.trigger_run_report(pid.clone());
        }

        // Await all promises concurrently
        let futures: Vec<_> = promise_ids.iter().map(|pid| async {
            let bytes = await_promise(pid).await;
            String::from_utf8(bytes).unwrap()
        }).collect();

        futures.join().await
    }
}
```

```rust
#[agent_definition]
pub trait RegionWorker {
    fn new(region: String) -> Self;
    fn run_report(&mut self, promise_id: PromiseId);
}

struct RegionWorkerImpl { region: String }

#[agent_implementation]
impl RegionWorker for RegionWorkerImpl {
    fn new(region: String) -> Self { Self { region } }

    fn run_report(&mut self, promise_id: PromiseId) {
        let report = format!("Report for {}: OK", self.region);
        complete_promise(&promise_id, report.as_bytes());
    }
}
```

### Error Handling with `TryJoin`

Use `TryJoin` to short-circuit on the first failure, or `Join` and handle errors manually for partial-failure tolerance:

These recipes use a separate fallible worker contract (implement it in the worker):

```rust
#[agent_definition]
pub trait FallibleWorker {
    fn new(id: u64) -> Self;
    async fn process(&self, item: String) -> Result<String, String>;
}
```

Only the returned domain `Result` is isolated here. Generated RPC clients panic on invocation
failure; `Join` does not turn those panics into domain errors or isolate them from the coordinator.

#### Short-circuit on first error (`TryJoin`)

```rust
use futures_concurrency::prelude::*;

async fn fan_out_strict(&mut self, items: Vec<String>) -> Result<Vec<String>, String> {
    let futures: Vec<_> = items.iter().enumerate().map(|(i, item)| {
        let child = FallibleWorkerClient::get(i as u64);
        let item = item.clone();
        async move {
            child.process(item).await
                .map_err(|e| format!("Worker {i} failed: {e}"))
        }
    }).collect();

    // Cancels remaining futures on first error
    futures.try_join().await
}
```

#### Collect partial results (`Join` + per-future error handling)

```rust
use futures_concurrency::prelude::*;

#[derive(Serialize, Deserialize)]
enum WorkResult {
    Success(String),
    Failure(String),
}

async fn fan_out_with_errors(&mut self, items: Vec<String>) -> (Vec<String>, Vec<String>) {
    let futures: Vec<_> = items.iter().enumerate().map(|(i, item)| {
        let child = FallibleWorkerClient::get(i as u64);
        let item = item.clone();
        async move {
            // Collect domain errors without cancelling siblings.
            match child.process(item.clone()).await {
                Ok(v) => WorkResult::Success(v),
                Err(e) => WorkResult::Failure(format!("Item {item} failed: {e}")),
            }
        }
    }).collect();

    let results = futures.join().await;

    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for r in results {
        match r {
            WorkResult::Success(v) => successes.push(v),
            WorkResult::Failure(e) => failures.push(e),
        }
    }
    (successes, failures)
}
```

## Approach 2: `fork()`

`fork()` clones the current agent at the current execution point, creating a new agent instance with the same state but a unique phantom ID. Only a durable agent can fork, and the forked agent is durable too. Use Golem promises to synchronize between the original and forked agents.

### Basic Fork Pattern

```rust
use golem_rust::{
    fork, ForkResult,
    create_promise, complete_promise, await_promise,
};

async fn parallel_compute(&mut self) -> Result<String, String> {
    let promise_id = create_promise();

    Ok(match fork().map_err(|e| format!("Fork failed: {e:?}"))? {
        ForkResult::Original(_details) => {
            // Wait for the forked agent to complete the promise
            let bytes = await_promise(&promise_id).await;
            let forked_result = String::from_utf8(bytes).unwrap();
            format!("Combined: original + {forked_result}")
        }
        ForkResult::Forked(_details) => {
            // Do work in the forked copy
            let result = "forked-result";
            complete_promise(&promise_id, result.as_bytes());
            "forked done".to_string() // Only seen by the forked agent
        }
    })
}
```

### Multi-Fork Fan-Out

Fork multiple times for N-way parallelism, then join all promises concurrently:

```rust
use futures_concurrency::prelude::*;
use golem_rust::{
    fork, ForkResult,
    create_promise, complete_promise, await_promise, PromiseId,
};

async fn multi_fork(&mut self, n: u32) -> Result<Vec<String>, String> {
    let promise_ids: Vec<PromiseId> = (0..n).map(|_| create_promise()).collect();

    for i in 0..n {
        match fork().map_err(|e| format!("Fork failed: {e:?}"))? {
            ForkResult::Original(_) => {
                // Continue to next fork
            }
            ForkResult::Forked(_) => {
                // Each forked agent does its slice of work
                let output = format!("result-from-fork-{i}");
                complete_promise(&promise_ids[i as usize], output.as_bytes());
                return Ok(vec![]); // Forked agent exits here
            }
        }
    }

    // Original agent collects all results concurrently
    let futures: Vec<_> = promise_ids.iter().map(|pid| async {
        let bytes = await_promise(pid).await;
        String::from_utf8(bytes).unwrap()
    }).collect();

    Ok(futures.join().await)
}
```

## When to Use Which Approach

| Criteria | Child Agents | `fork()` |
|----------|-------------|----------|
| Work is **independent** and stateless | ✅ Best fit | Works but overkill |
| Need to **share current state** with workers | ❌ Must pass via args | ✅ Forked copy inherits state |
| Workers need **persistent identity** | ✅ Each has own ID | ❌ Each forked agent gets a generated phantom ID |
| Number of parallel tasks is **dynamic** | ✅ Spawn as many as needed | ✅ Fork in a loop |
| Need **domain error collection** | Return `Result` from the worker; RPC failures can still panic in the caller | Forked agent shares oplog lineage |

## Key Points

- **No CPU threads**: Async tasks can interleave within an invocation; distribute CPU work across agent instances
- **Durability**: All RPC calls, promises, and fork operations are durably recorded — work survives crashes
- **Deadlock avoidance**: Never have two agents awaiting each other synchronously — use `trigger_` to break cycles
- **Cleanup**: Child agents persist after the coordinator finishes; delete them explicitly if they hold unwanted state
- **`futures_concurrency`**: Use `Vec<Future>.join().await` to await all futures concurrently, or `.try_join().await` to short-circuit on the first error
- **Always async**: Prefer `await_promise` / `await_promise_json` over blocking variants for all concurrent patterns
