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

// Type-only coverage for values crossing the package's public entrypoints.
// Checked by the package typecheck script; NOT executed by vitest.

import { AgentId, ComponentId, Uuid } from '../dist/index.mjs';
import { v } from '../dist/schema.mjs';

const componentId = new ComponentId(new Uuid(1n, 2n));
const id = AgentId.create({
  componentId,
  typeName: 'ExampleAgent',
  constructorValue: v.record([v.string('example')]),
});
AgentId.parse(id);
