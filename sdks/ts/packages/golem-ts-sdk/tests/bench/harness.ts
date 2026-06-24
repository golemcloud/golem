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

// Runtime-agnostic benchmark facade. The `*.bench.ts` suites import `bench` and
// `describe` from here instead of directly from `vitest` so the exact same
// suites can run under two runtimes:
//
//   * Node / V8  — `vitest bench` picks up the `*.bench.ts` files and this
//     module forwards to Vitest's real benchmarking API.
//   * QuickJS    — the QuickJS bench bundle (see `scripts/bench-quickjs.mjs`)
//     rewrites this import to a lightweight registry
//     (`tests/bench/quickjs/registry.ts`) and times the cases inside the
//     wasm-rquickjs runtime, which is what the SDK actually runs on in
//     production.
//
// Keeping a single set of suites avoids the two runtimes drifting apart.

export { bench, describe } from 'vitest';
