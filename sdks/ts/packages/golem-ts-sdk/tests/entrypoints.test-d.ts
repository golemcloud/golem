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
  AgentId,
  DynamicAgentClient,
  InvocationId,
  RemoteCallError,
  Uuid,
  defineAgentClient,
  method,
} from '../dist/index.mjs';
import { getAgentType } from '../dist/reflection.mjs';
import { z } from 'zod';
import { v } from '../dist/schema.mjs';
import * as durableStreams from 'golem:agent/durable-streams@2.0.0';
import * as agentHost from 'golem:agent/host@2.0.0';

const reader = new durableStreams.DurableStreamReader(
  { url: 'https://streams.example', mode: 'json', timeoutMs: 30000n },
  undefined,
);
const writer = new durableStreams.DurableStreamWriter(
  {
    url: 'https://streams.example',
    contentType: 'application/json',
    producerId: 'typed',
    producerEpoch: 0n,
    timeoutMs: 30000n,
  },
  undefined,
);
const readBatch = reader.read({ checkpoint: { offset: '-1' }, transport: 'catch-up' });
const appendReceipt = writer.append({
  payload: { tag: 'json', val: ['17'] },
  sequence: 0n,
  close: false,
});
readBatch satisfies Promise<durableStreams.DurableStreamBatch>;
appendReceipt satisfies Promise<durableStreams.DurableStreamAppendReceipt>;
// @ts-expect-error Durable Streams operations require a resource
durableStreams.readDurableStreamBatch;
// @ts-expect-error Durable Streams operations require a resource
durableStreams.appendDurableStreamBatch;
// @ts-expect-error Durable Streams operations belong to their dedicated interface
agentHost.readDurableStreamBatch;
// @ts-expect-error Durable Streams operations belong to their dedicated interface
agentHost.appendDurableStreamBatch;

const agentId = AgentId.parse('00000007-0000-8000-8000-000000000011');
agentId.uuid satisfies Uuid;
agentId.toString() satisfies string;
agentId.equals(new AgentId(new Uuid(1n, 2n))) satisfies boolean;
// @ts-expect-error agent IDs do not derive clients
agentId.client;
// @ts-expect-error agent IDs do not expose their constructor parameters
agentId.parts;
// @ts-expect-error reflected agent types no longer name the implementing component
getAgentType('ExampleAgent')!.implementedBy;
getAgentType('ExampleAgent')!.client.getById(agentId) satisfies Promise<unknown>;
getAgentType('ExampleAgent')!
  .client.upsert({ name: 'example' })
  .agentId() satisfies Promise<AgentId>;

const definition = defineAgentClient({
  methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
});
definition.getById(agentId).then((client) => client.ping({ message: 'hello' }));
// @ts-expect-error method-only clients address agents by ID only
definition.upsert;
DynamicAgentClient.getById(agentId).then((client) =>
  client.method('ping').invokeValue(v.record([])),
);
// @ts-expect-error dynamic clients are only connected through getById
new DynamicAgentClient(agentId);

const fullDefinition = defineAgentClient({
  name: 'ExampleAgent',
  id: { name: z.string() },
  methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
});
const upserted = fullDefinition.client.upsert({ name: 'example' });
upserted.ping({ message: 'upsert' });
upserted.agentId() satisfies Promise<AgentId>;
fullDefinition.client
  .create({ name: 'example' }, { keyed: true, env: { MODE: 'test' } })
  .then((client) => client.ping({ message: 'create' }));
fullDefinition.client.getById(agentId).then((client) => client.agentId());
// @ts-expect-error upsert takes no configuration overrides
fullDefinition.client.upsert({ name: 'example' }, { config: {} });
// @ts-expect-error env overrides are string values
fullDefinition.client.create({ name: 'example' }, { env: { PORT: 1 } });
// @ts-expect-error agent IDs come from the host, not from constructor parameters
fullDefinition.agentId;
// @ts-expect-error the phantom factories are removed
fullDefinition.client.newPhantom;
// @ts-expect-error the old keyed factory is removed
fullDefinition.client.get;
new RemoteCallError('typed', { tag: 'conflict', details: 'exists' }) satisfies RemoteCallError;

const ephemeralDefinition = defineAgentClient({
  name: 'EphemeralExampleAgent',
  mode: 'ephemeral',
  id: { name: z.string() },
  methods: { ping: method({ input: {}, returns: z.string() }) },
});
// @ts-expect-error ephemeral types cannot be created by ID
ephemeralDefinition.client.create;
// @ts-expect-error ephemeral types cannot be addressed by ID
ephemeralDefinition.client.getById;
const ephemeralClient = ephemeralDefinition.client.upsert({ name: 'example' });
// @ts-expect-error ephemeral clients have no stable agent ID
ephemeralClient.agentId;
ephemeralClient.ping().then(({ metadata, value }) => {
  metadata.agentId satisfies AgentId;
  metadata.invocationId satisfies InvocationId;
  value satisfies string;
});

// @ts-expect-error lifecycle mode requires a complete name + id definition
defineAgentClient({ mode: 'ephemeral', methods: definition.methods });
// @ts-expect-error method-only clients cannot declare a name without an ID shape
defineAgentClient({ name: 'NamedContract', methods: definition.methods });
// @ts-expect-error method-only clients cannot declare a name or lifecycle mode
defineAgentClient({ name: 'NamedContract', mode: 'durable', methods: definition.methods });
