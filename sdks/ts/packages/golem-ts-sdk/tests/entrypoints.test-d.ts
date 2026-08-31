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

import { AgentId, ComponentId, Uuid, defineAgentClient, method } from '../dist/index.mjs';
import { z } from 'zod';
import { v } from '../dist/schema.mjs';

const componentId = new ComponentId(new Uuid(1n, 2n));
const id = AgentId.create({
  componentId,
  typeName: 'ExampleAgent',
  constructorValue: v.record([v.string('example')]),
});
AgentId.parse(id);
const contract = defineAgentClient({
  methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
});
id.client(contract).ping({ message: 'hello' });
id.dynamicClient().method('ping').invokeValue(v.record([]));

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
// @ts-expect-error name-only binding contracts are lifecycle-free
defineAgentClient({ name: 'NamedContract', mode: 'durable', methods: contract.methods });
