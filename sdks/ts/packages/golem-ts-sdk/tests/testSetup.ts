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

import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src/agentClassName';

// The production runtime boundary now targets `golem:agent/host@2.0.0`. Mock it
// with a schema-native, JSON-round-trippable `make-agent-id` / `parse-agent-id`
// pair (the constructor params crossing the boundary are `schema-value-tree`s)
// plus the registry and RPC stubs the SDK touches.
vi.mock('golem:agent/host@2.0.0', () => ({
  getAllAgentTypes: () => [],
  getAgentType: (agentTypeName: string) => {
    if (agentTypeName === 'FooAgent') {
      const agentType = AgentTypeRegistry.get(new AgentClassName('FooAgent'));
      if (!agentType) {
        throw new Error('Missing FooAgent in registry');
      }
      return {
        agentType,
        implementedBy: { uuid: { lowBits: BigInt(0), highBits: BigInt(0) } },
      };
    }
    return undefined;
  },
  makeAgentId: (
    agentTypeName: string,
    input: unknown,
    phantomId: { highBits: bigint; lowBits: bigint } | undefined,
  ): string => {
    const phantomPostfix = phantomId ? `[${phantomId.highBits}-${phantomId.lowBits}]` : '';
    return `${agentTypeName}(${JSON.stringify(input)})${phantomPostfix}`;
  },
  parseAgentId: (agentId: string) => {
    const match = agentId.match(/^(.*)\((.*)\)(\[(\d+)-(\d+)])?$/);
    if (!match) {
      throw new Error(`Invalid agent ID: ${agentId}`);
    }
    const [, typeName, inputJson, , hiBits, loBits] = match;
    // The graph is a structurally-required placeholder; the SDK only reads the
    // value tree and re-derives types from its own registry.
    const typed = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: JSON.parse(inputJson),
    };
    let phantomId: { highBits: bigint; lowBits: bigint } | undefined = undefined;
    if (hiBits !== undefined) {
      phantomId = { highBits: BigInt(hiBits), lowBits: BigInt(loBits) };
    }
    return [typeName, typed, phantomId];
  },
  getConfigValue: () => {
    throw new Error('getConfigValue is not mocked in this test setup');
  },
  createWebhook: () => 'https://example.com/webhook',
  WasmRpc: vi.fn().mockImplementation(() => ({
    invokeAndAwait: vi.fn(),
    invoke: vi.fn(),
    asyncInvokeAndAwait: vi.fn(),
    scheduleInvocation: vi.fn(),
    scheduleCancelableInvocation: vi.fn(),
  })),
}));

vi.mock('golem:core/types@2.0.0', () => ({
  parseUuid: (uuid: string) => {
    const parts = uuid.replace(/-/g, '');
    return {
      highBits: BigInt('0x' + parts.slice(0, 16)),
      lowBits: BigInt('0x' + parts.slice(16)),
    };
  },
  uuidToString: (uuid: { highBits: bigint; lowBits: bigint }) => {
    const hi = BigInt.asUintN(64, uuid.highBits).toString(16).padStart(16, '0');
    const lo = BigInt.asUintN(64, uuid.lowBits).toString(16).padStart(16, '0');
    const hex = hi + lo;
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  },
}));

vi.mock('golem:api/oplog@1.5.0', () => ({
  GetOplog: vi.fn(),
  SearchOplog: vi.fn(),
  enrichOplogEntries: vi.fn(),
}));

vi.mock('golem:quota/types@1.5.0', () => ({}));

vi.mock('golem:secrets/reveal@0.1.0', () => ({
  reveal: () => {
    throw new Error('reveal is not mocked in this test setup');
  },
}));

(globalThis as any).currentAgentId = 'foo-agent(123)';

vi.mock('wasi:cli/environment@0.2.3', () => ({
  getEnvironment: () => [['GOLEM_AGENT_ID', (globalThis as any).currentAgentId]],
}));

// Load the package barrel so its side-effecting imports register the schema
// walkers (zod / valibot / arktype / effect) for tests that import fluent
// submodules directly rather than the top-level entry.
await import('../src');
