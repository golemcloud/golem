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

import {
  ComponentId,
  ParsedAgentId,
  Uuid,
  defineAgentClient,
  method,
  type AgentId,
} from '../dist/index.mjs';
import { ComponentId as ReflectionComponentId, getAgentType } from '../dist/reflection.mjs';
import { z } from 'zod';
import { v } from '../dist/schema.mjs';

const componentId = new ComponentId(new Uuid(1n, 2n));
const reflectionComponentId: ReflectionComponentId = componentId;
getAgentType('ExampleAgent')!.implementedBy satisfies ReflectionComponentId;
reflectionComponentId satisfies ComponentId;
const id = ParsedAgentId.create({
  typeName: 'ExampleAgent',
  constructorValue: v.record([v.string('example')]),
});
id.parts();
const managementId: AgentId = { componentId, agentId: id.value };
managementId.componentId satisfies ComponentId;
// @ts-expect-error management IDs do not provide reflection client helpers
managementId.client;
const contract = defineAgentClient({
  methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
});
id.client(contract).ping({ message: 'hello' });
id.dynamicClient().method('ping').invokeValue(v.record([]));

const exactContract = defineAgentClient({
  name: 'ExampleAgent',
  id: { name: z.string() },
  methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
});
const schemaLibraryId = exactContract.agentId({ name: 'example' });
const schemaValueId = ParsedAgentId.create({
  typeName: exactContract.name,
  constructorValue: v.record([v.string('example')]),
});
schemaLibraryId.client(exactContract).ping({ message: 'schema library' });
schemaValueId.client(exactContract).ping({ message: 'schema value' });
schemaValueId.value satisfies string;

const ephemeralContract = defineAgentClient({
  name: 'EphemeralExampleAgent',
  mode: 'ephemeral',
  id: { name: z.string() },
  methods: { ping: method({ input: {}, returns: z.string() }) },
});
ephemeralContract.client
  .newPhantom({ name: 'example' })
  .ping()
  .then(({ metadata, value }) => {
    metadata.agentId satisfies string;
    metadata.idempotencyKey satisfies string;
    value satisfies string;
  });

// @ts-expect-error lifecycle mode requires a complete exact name + id definition
defineAgentClient({ mode: 'ephemeral', methods: contract.methods });
// @ts-expect-error binding-only contracts cannot declare a name without an ID shape
defineAgentClient({ name: 'NamedContract', methods: contract.methods });
// @ts-expect-error binding-only contracts cannot declare a name or lifecycle mode
defineAgentClient({ name: 'NamedContract', mode: 'durable', methods: contract.methods });
