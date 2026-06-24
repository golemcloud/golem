// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Minimal benchmark timer for the QuickJS runtime. wasm-rquickjs only exposes a
// millisecond-resolution clock (`Date.now()`), so each case is run for a fixed
// time window and the iteration count is used to derive throughput. Iterations
// are executed in growing batches to keep the `Date.now()` polling overhead
// negligible relative to the measured work.

import { RegisteredBench } from './registry';

export interface BenchResult {
  group: string;
  name: string;
  /** Operations per second. */
  hz: number;
  /** Mean nanoseconds per operation. */
  nsPerOp: number;
  iterations: number;
  elapsedMs: number;
}

export interface RunOptions {
  /** Minimum wall-clock time to spend measuring each case, in milliseconds. */
  minTimeMs?: number;
  /** Minimum wall-clock time to spend warming up each case, in milliseconds. */
  warmupMs?: number;
}

function spin(fn: () => unknown, minTimeMs: number): { iterations: number; elapsedMs: number } {
  let iterations = 0;
  let batch = 1;
  const start = Date.now();
  while (Date.now() - start < minTimeMs) {
    for (let i = 0; i < batch; i++) {
      fn();
    }
    iterations += batch;
    if (batch < 4096) {
      batch *= 2;
    }
  }
  return { iterations, elapsedMs: Date.now() - start };
}

export function runBenches(benches: RegisteredBench[], options: RunOptions = {}): BenchResult[] {
  const minTimeMs = options.minTimeMs ?? 1000;
  const warmupMs = options.warmupMs ?? 200;

  const results: BenchResult[] = [];
  for (const b of benches) {
    spin(b.fn, warmupMs);
    const { iterations, elapsedMs } = spin(b.fn, minTimeMs);
    const hz = (iterations * 1000) / elapsedMs;
    results.push({
      group: b.group,
      name: b.name,
      hz,
      nsPerOp: elapsedMs === 0 ? 0 : (elapsedMs * 1e6) / iterations,
      iterations,
      elapsedMs,
    });
  }
  return results;
}
