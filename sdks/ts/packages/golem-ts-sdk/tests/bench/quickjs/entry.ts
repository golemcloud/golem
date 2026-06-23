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

// QuickJS benchmark entry point. This module is bundled (with the `harness`
// import rewritten to `./registry`) and wrapped into a wasm component by
// `scripts/bench-quickjs.mjs`. Importing each `*.bench.ts` suite registers its
// cases into the shared registry; `run` then times them under QuickJS and
// returns the results as JSON.

import { registeredBenches } from './registry';
import { runBenches } from './run';

// Side-effect imports: each suite registers its cases on import.
import '../conversion.bench';
import '../largeInput.bench';
import '../largeNested.bench';
import '../configSchema.bench';

export function run(): string {
  const results = runBenches(registeredBenches());
  return JSON.stringify({ runtime: 'quickjs', results });
}
