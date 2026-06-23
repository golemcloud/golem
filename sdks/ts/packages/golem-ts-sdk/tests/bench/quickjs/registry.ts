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

// Drop-in replacement for the Vitest `bench` / `describe` API used by the
// `*.bench.ts` suites, for running them under QuickJS. The QuickJS bench bundle
// aliases `tests/bench/harness` to this module, so importing a suite simply
// records its cases into the registry instead of scheduling them in Vitest.
// `tests/bench/quickjs/run.ts` then times each recorded case.

export interface RegisteredBench {
  group: string;
  name: string;
  fn: () => unknown;
}

const registry: RegisteredBench[] = [];
let currentGroup = '';

// Mirrors Vitest's `describe`: runs the body synchronously, scoping any `bench`
// calls inside it to `name`. Suites only ever register cases synchronously.
export function describe(name: string, body: () => void): void {
  const previous = currentGroup;
  currentGroup = name;
  try {
    body();
  } finally {
    currentGroup = previous;
  }
}

// Mirrors Vitest's `bench`: records the case. The options bag (e.g. `{ time }`)
// is accepted for signature compatibility but ignored — the QuickJS runner has
// its own timing policy in `run.ts`.
export function bench(name: string, fn: () => unknown, _options?: { time?: number }): void {
  registry.push({ group: currentGroup, name, fn });
}

export function registeredBenches(): RegisteredBench[] {
  return registry;
}
