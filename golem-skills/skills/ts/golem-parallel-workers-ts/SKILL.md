---
name: golem-parallel-workers-ts
description: "Fan out work to multiple parallel agents and collect results in a TypeScript Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning child agents for parallel work, forking, or aggregating results from multiple agents."
---

# Parallel Workers — Fan-Out / Fan-In (TypeScript)

## Overview

Golem agents process invocations **sequentially** — a single agent cannot run work in parallel. To execute work concurrently, distribute it across **multiple agent instances**. This skill covers two approaches:

1. **Child agents via `clientFor(AgentDef)(id)`** — spawn separate agent instances, dispatch work, and collect results
2. **`fork()`** — clone the current agent at the current execution point for lightweight parallel execution

## Approach 1: Child Agent Fan-Out

Spawn child agents, call them concurrently with `Promise.all`, and aggregate results.

### Basic Pattern

```typescript
import { z } from 'zod';
import { defineAgent, method, clientFor } from '@golemcloud/golem-ts-sdk';

export const Worker = defineAgent({
    name: 'Worker',
    id: { id: z.number() },
    methods: {
        process: method({ input: { data: z.string() }, returns: z.string() }),
    },
});

export const WorkerImpl = Worker.implement({
    init: ({ id }) => ({ id: id.id }),
    methods: {
        process({ data }) {
            return `processed-${data}`;
        },
    },
});

// A typed RPC client factory for the remote Worker (built once, caches codecs).
const workerClient = clientFor(Worker);

export const Coordinator = defineAgent({
    name: 'Coordinator',
    id: { name: z.string() },
    methods: {
        fanOut: method({ input: { items: z.array(z.string()) }, returns: z.array(z.string()) }),
    },
});

export const CoordinatorImpl = Coordinator.implement({
    init: () => ({}),
    methods: {
        async fanOut({ items }) {
            // Spawn one child per item and call concurrently.
            const promises = items.map((item, i) => workerClient({ id: i }).process({ data: item }));
            // Wait for all children to finish.
            return await Promise.all(promises);
        },
    },
});
```

### Chunked Fan-Out

When spawning many children, batch them to limit concurrency:

```typescript
async fanOutChunked({ ids }) {
    const chunks = arrayChunks(ids, 5); // Process 5 at a time
    const results: number[] = [];

    for (const chunk of chunks) {
        const promises = chunk.map((id) => workerClient({ id }).compute({ n: id }));
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

For long-running work, trigger children with `.trigger()` and collect results via
Golem promises. A `PromiseId` is a nested record of bigints, so ship it across the
agent boundary as a bigint-aware JSON string:

```typescript
import { z } from 'zod';
import {
    defineAgent, method, clientFor,
    createPromise, awaitPromise, completePromise, PromiseId,
} from '@golemcloud/golem-ts-sdk';

export function encodePromiseId(id: PromiseId): string {
    return JSON.stringify(id, (_k, v) => (typeof v === 'bigint' ? { '#bigint': v.toString() } : v));
}
export function decodePromiseId(text: string): PromiseId {
    return JSON.parse(text, (_k, v) =>
        v && typeof v === 'object' && '#bigint' in v ? BigInt(v['#bigint']) : v,
    ) as PromiseId;
}

export const RegionWorker = defineAgent({
    name: 'RegionWorker',
    id: { region: z.string() },
    methods: {
        runReport: method({ input: { promiseId: z.string() }, returns: z.void() }),
    },
});

export const RegionWorkerImpl = RegionWorker.implement({
    init: ({ id }) => ({ region: id.region }),
    methods: {
        runReport({ promiseId }) {
            const report = `Report for ${this.region}: OK`;
            completePromise(decodePromiseId(promiseId), new TextEncoder().encode(report));
        },
    },
});

const regionClient = clientFor(RegionWorker);

// Inside a coordinator method handler:
async dispatchAndCollect({ regions }) {
    // Create one promise per child.
    const promiseIds = regions.map(() => createPromise());

    // Fire-and-forget: trigger each child with its (encoded) promise ID.
    regions.forEach((region, i) => {
        regionClient({ region }).runReport.trigger({ promiseId: encodePromiseId(promiseIds[i]) });
    });

    // Collect all results (the agent suspends until each promise completes).
    return await Promise.all(
        promiseIds.map(async (pid) => new TextDecoder().decode(await awaitPromise(pid))),
    );
}
```

### Error Handling

Use `Promise.allSettled` to handle partial failures:

```typescript
async fanOutWithErrors({ items }) {
    const promises = items.map((item, i) => workerClient({ id: i }).process({ data: item }));
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
import { z } from 'zod';
import {
    defineAgent, method,
    fork, createPromise, awaitPromise, completePromise,
} from '@golemcloud/golem-ts-sdk';

export const ForkAgent = defineAgent({
    name: 'ForkAgent',
    id: { name: z.string() },
    methods: {
        parallelCompute: method({ input: {}, returns: z.string() }),
    },
});

export const ForkAgentImpl = ForkAgent.implement({
    init: () => ({}),
    methods: {
        async parallelCompute() {
            const promiseId = createPromise();

            const result = fork();
            switch (result.tag) {
                case 'original': {
                    // Wait for the forked agent to complete the promise.
                    const bytes = await awaitPromise(promiseId);
                    const forkedResult = new TextDecoder().decode(bytes);
                    return `Combined: original + ${forkedResult}`;
                }
                case 'forked': {
                    // Do work in the forked copy.
                    const computed = 'forked-result';
                    completePromise(promiseId, new TextEncoder().encode(computed));
                    return 'forked done'; // This return is only seen by the forked agent.
                }
            }
        },
    },
});
```

### Multi-Fork Fan-Out

Fork multiple times for N-way parallelism:

```typescript
async multiFork({ n }) {
    const promiseIds = Array.from({ length: n }, () => createPromise());

    for (let i = 0; i < n; i++) {
        const result = fork();
        if (result.tag === 'forked') {
            // Each forked agent does its slice of work.
            const output = `result-from-fork-${i}`;
            completePromise(promiseIds[i], new TextEncoder().encode(output));
            return []; // Forked agent exits here.
        }
    }

    // Original agent collects all results.
    return await Promise.all(
        promiseIds.map(async (pid) => new TextDecoder().decode(await awaitPromise(pid))),
    );
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
