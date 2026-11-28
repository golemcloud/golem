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

// The production runtime boundary targets `golem:agent/host@2.0.0`. Mock its registry
// and RPC surfaces; tests that inspect an agent ID configure the host result they expect.
export const MOCK_REMOTE_AGENT_ID = {
  uuid: { highBits: 0x00000007_0000_8000n, lowBits: 0x8000_0000_0000_0004n },
};

function makeWasmRpc() {
  return {
    agentId: vi.fn(async () => MOCK_REMOTE_AGENT_ID),
    invokeAndAwait: vi.fn(),
    trigger: vi.fn(),
    schedule: vi.fn(),
  };
}

const MockWasmRpc = Object.assign(vi.fn(), {
  bindSelf: vi.fn(),
  upsert: vi.fn(makeWasmRpc),
  getById: vi.fn(makeWasmRpc),
  create: vi.fn(async () => ({ client: makeWasmRpc(), agentId: MOCK_REMOTE_AGENT_ID })),
});

vi.mock('golem:agent/durable-streams@2.0.0', () => ({
  DurableStreamReader: vi.fn(),
  DurableStreamWriter: vi.fn(),
}));

vi.mock('golem:agent/host@2.0.0', () => ({
  getAllAgentTypes: vi.fn(() => []),
  getAgentType: vi.fn((agentTypeName: string) => {
    if (agentTypeName === 'FooAgent') {
      const agentType = AgentTypeRegistry.get(new AgentClassName('FooAgent'));
      if (!agentType) {
        throw new Error('Missing FooAgent in registry');
      }
      return { agentType };
    }
    return undefined;
  }),
  getAgentTypeByAgentId: vi.fn(() => undefined),
  lookupAgentId: vi.fn(() => undefined),
  getConfigValue: vi.fn(() => {
    throw new Error('getConfigValue is not mocked in this test setup');
  }),
  createWebhook: () => 'https://example.com/webhook',
  WasmRpc: MockWasmRpc,
  Invocation: { get: vi.fn() },
}));

// Entry points read the caller from the invocation context. Tests that need a
// specific caller override `caller` with `mockReturnValue`.
vi.mock('golem:agent/invocation-context@2.0.0', () => ({
  invocationId: vi.fn(() => undefined),
  caller: vi.fn(() => ({
    original: { tag: 'anonymous' },
    immediate: { tag: 'anonymous' },
    tokens: [],
  })),
}));

vi.mock('golem:tool/host@0.1.0', () => {
  const rpc = vi.fn(function () {
    return {
      invokeAndAwait: vi.fn(),
      invoke: vi.fn(),
      asyncInvokeAndAwait: vi.fn(),
    };
  });
  return {
    createStdin: vi.fn(),
    createOutput: vi.fn(),
    ToolRpc: Object.assign(rpc, { create: rpc }),
  };
});

vi.mock('golem:core/types@2.0.0', () => ({
  // The host permission-card resource class. Wire lifting accepts only its
  // instances; tests build fakes with `fakeRawCard`.
  PermissionCard: class PermissionCard {},
  SchemaValueStream: {
    unwrap: async (value: { reader: AsyncIterable<unknown>; outcome?: Promise<unknown> }) => [
      value.reader,
      value.outcome ?? Promise.resolve({ tag: 'ok' }),
    ],
    wrap: async (
      reader: AsyncIterable<unknown> & { failWrap?: boolean; onDispose?: () => void },
      outcome: Promise<unknown>,
    ) => {
      if (reader.failWrap) throw new Error('test schema value stream wrap failure');
      return { reader, outcome, [Symbol.dispose]: () => reader.onDispose?.() };
    },
  },
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

vi.mock('golem:api/host@1.5.0', () => ({
  getSelfMetadata: vi.fn(() => ({
    agentId: { uuid: { highBits: 0x00000007_0000_8000n, lowBits: 0x8000_0000_0000_0001n } },
    agentTypeName: 'TestAgent',
    fingerprint: { highBits: 0n, lowBits: 3n },
    args: [],
    env: [],
    config: [],
    status: 'idle',
    componentRevision: 0n,
    retryCount: 0n,
    environmentId: { uuid: { highBits: 0n, lowBits: 2n } },
  })),
}));

vi.mock('golem:api/oplog@1.5.0', () => ({
  GetOplog: vi.fn(),
  SearchOplog: vi.fn(),
  enrichOplogEntries: vi.fn(),
}));

vi.mock('golem:quota/types@1.5.0', () => ({}));
vi.mock('golem:permissions/types@0.1.0', () => ({}));
vi.mock('golem:permissions/inspect@0.1.0', () => ({}));
vi.mock('golem:permissions/derive@0.1.0', () => ({}));
vi.mock('golem:permissions/revoke@0.1.0', () => ({}));
vi.mock('golem:permissions/wallet@0.1.0', () => ({}));
vi.mock('golem:permissions/kernel-introspection@0.1.0', () => ({}));

vi.mock('golem:secrets/reveal@0.1.0', () => ({
  reveal: vi.fn(() => {
    throw new Error('reveal is not mocked in this test setup');
  }),
}));

(globalThis as any).currentAgentId = '00000007-0000-8000-8000-000000000001';
(globalThis as any).currentAgentType = 'FooAgent';

vi.mock('wasi:cli/environment@0.3.0', () => ({
  getEnvironment: () =>
    (
      [
        ['GOLEM_AGENT_ID', (globalThis as any).currentAgentId],
        ['GOLEM_AGENT_TYPE', (globalThis as any).currentAgentType],
      ] as [string, string | undefined][]
    ).filter((entry): entry is [string, string] => entry[1] !== undefined),
}));

// Load the package barrel so its side-effecting imports register the schema
// walkers (zod / valibot / arktype / effect) for tests that import the SDK entry point
// submodules directly rather than the top-level entry.
await import('../src');
