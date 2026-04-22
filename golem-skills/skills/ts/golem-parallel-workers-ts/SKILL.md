---
name: golem-parallel-workers-ts
description: "Fan out work to multiple parallel agents and collect results in a TypeScript Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning child agents for parallel work, forking, or aggregating results from multiple agents."
---

# Parallel Workers — Fan-Out / Fan-In (TypeScript)

## Overview

Golem agents process invocations **sequentially** — a single agent cannot run work in parallel. To execute work concurrently, distribute it across **multiple agent instances**. This skill covers two approaches:

1. **Child agents via `AgentClass.get(id)`** — spawn separate agent instances, dispatch work, and collect results
2. **`fork()`** — clone the current agent at the current execution point for lightweight parallel execution

## Approach 1: Child Agent Fan-Out

Spawn child agents, call them concurrently with `Promise.all`, and aggregate results.

### Basic Pattern

```typescript
import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';

@agent()
class Coordinator extends BaseAgent {
    constructor() { super(); }

    async fanOut(items: string[]): Promise<string[]> {
        // Spawn one child per item and call concurrently
        const promises = items.map(async (item, i) => {
            const child = Worker.get(i);
            return await child.process(item);
        });

        // Wait for all children to finish
        return await Promise.all(promises);
    }
}

@agent()
class Worker extends BaseAgent {
    private readonly id: number;
    constructor(id: number) { super(); this.id = id; }

    async process(data: string): Promise<string> {
        return `processed-${data}`;
    }
}
```

### Chunked Fan-Out

When spawning many children, batch them to limit concurrency:

```typescript
async fanOutChunked(ids: number[]): Promise<number[]> {
    const chunks = arrayChunks(ids, 5); // Process 5 at a time
    const results: number[] = [];

    for (const chunk of chunks) {
        const promises = chunk.map(async id => {
            return await Worker.get(id).compute(id);
        });
        results.push(...await Promise.all(promises));
    }
    return results;
}

function arrayChunks<T>(arr: T[], size: number): T[][] {
    const chunks: T[][] = [];
    for (let i = 0; i < arr.length; i += size) {
        chunks.push(arr.slice(i, i + size));
    }
    return chunks;
}
```

### Fire-and-Forget with Promise Collection

For long-running work, trigger children with fire-and-forget and collect results via Golem promises:

```typescript
import {
    BaseAgent, agent,
    createPromise, awaitPromise, completePromise,
    PromiseId,
} from '@golemcloud/golem-ts-sdk';

@agent()
class Coordinator extends BaseAgent {
    constructor() { super(); }

    async dispatchAndCollect(regions: string[]): Promise<string[]> {
        // Create one promise per child
        const promiseIds = regions.map(() => createPromise());

        // Fire-and-forget: trigger each child with its promise ID
        regions.forEach((region, i) => {
            RegionWorker.get(region).runReport.trigger(promiseIds[i]);
        });

        // Collect all results (agent suspends until each promise completes)
        const results = await Promise.all(
            promiseIds.map(async pid => {
                const bytes = await awaitPromise(pid);
                return new TextDecoder().decode(bytes);
            })
        );

        return results;
    }
}

@agent()
class RegionWorker extends BaseAgent {
    private readonly region: string;
    constructor(region: string) { super(); this.region = region; }

    async runReport(promiseId: PromiseId): Promise<void> {
        const report = `Report for ${this.region}: OK`;
        completePromise(promiseId, new TextEncoder().encode(report));
    }
}
```

### Error Handling

Use `Promise.allSettled` to handle partial failures:

```typescript
async fanOutWithErrors(items: string[]): Promise<{ successes: string[]; failures: string[] }> {
    const promises = items.map(async (item, i) => {
        const child = Worker.get(i);
        return await child.process(item);
    });

    const settled = await Promise.allSettled(promises);

    const successes: string[] = [];
    const failures: string[] = [];

    settled.forEach((result, i) => {
        if (result.status === 'fulfilled') {
            successes.push(result.value);
        } else {
            failures.push(`Item ${items[i]} failed: ${result.reason}`);
        }
    });

    return { successes, failures };
}
```

## Approach 2: `fork()`

`fork()` clones the current agent at the current execution point, creating a new agent instance with the same state but a unique phantom ID. Use Golem promises to synchronize between the original and forked agents.

### Basic Fork Pattern

```typescript
import {
    BaseAgent, agent,
    fork, createPromise, awaitPromise, completePromise,
} from '@golemcloud/golem-ts-sdk';

@agent()
class ForkAgent extends BaseAgent {
    constructor() { super(); }

    async parallelCompute(): Promise<string> {
        const promiseId = createPromise();

        const result = fork();
        switch (result.tag) {
            case 'original':
                // Wait for the forked agent to complete the promise
                const bytes = await awaitPromise(promiseId);
                const forkedResult = new TextDecoder().decode(bytes);
                return `Combined: original + ${forkedResult}`;

            case 'forked':
                // Do work in the forked copy
                const computed = "forked-result";
                completePromise(promiseId, new TextEncoder().encode(computed));
                return "forked done"; // This return is only seen by the forked agent
        }
    }
}
```

### Multi-Fork Fan-Out

Fork multiple times for N-way parallelism:

```typescript
async multiFork(n: number): Promise<string[]> {
    const promiseIds = Array.from({ length: n }, () => createPromise());

    for (let i = 0; i < n; i++) {
        const result = fork();
        if (result.tag === 'forked') {
            // Each forked agent does its slice of work
            const output = `result-from-fork-${i}`;
            completePromise(promiseIds[i], new TextEncoder().encode(output));
            return []; // Forked agent exits here
        }
    }

    // Original agent collects all results
    const results = await Promise.all(
        promiseIds.map(async pid => {
            const bytes = await awaitPromise(pid);
            return new TextDecoder().decode(bytes);
        })
    );

    return results;
}
```

## When to Use Which Approach

| Criteria | Child Agents | `fork()` |
|----------|-------------|----------|
| Work is **independent** and stateless | ✅ Best fit | Works but overkill |
| Need to **share current state** with workers | ❌ Must pass via args | ✅ Forked copy inherits state |
| Workers need **persistent identity** | ✅ Each has own ID | ❌ Forked agents are ephemeral phantoms |
| Number of parallel tasks is **dynamic** | ✅ Spawn as many as needed | ✅ Fork in a loop |
| Need **simple error isolation** | ✅ Child failure doesn't crash parent | ⚠️ Forked agent shares oplog lineage |

## Key Points

- **No threads**: Golem is single-threaded per agent — parallelism is achieved by distributing across agent instances
- **Durability**: All RPC calls, promises, and fork operations are durably recorded — work survives crashes
- **Deadlock avoidance**: Never have two agents awaiting each other synchronously — use `.trigger()` to break cycles
- **Cleanup**: Child agents persist after the coordinator finishes; delete them explicitly if they hold unwanted state
