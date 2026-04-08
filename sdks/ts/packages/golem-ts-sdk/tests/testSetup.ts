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

import { DataValue, RegisteredAgentType, Uuid, WasmRpc } from 'golem:agent/host@1.5.0';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src';

vi.mock('golem:agent/host@1.5.0', () => ({
  getAgentType: (agentTypeName: string): RegisteredAgentType | undefined => {
    if (agentTypeName == 'FooAgent') {
      const agentType = AgentTypeRegistry.get(new AgentClassName('FooAgent'));

      if (!agentType) {
        throw new Error('Missing FooAgent in registry');
      }

      return {
        agentType: agentType,
        implementedBy: {
          uuid: {
            lowBits: BigInt(0),
            highBits: BigInt(0),
          },
        },
      };
    } else {
      return undefined;
    }
  },
  makeAgentId: (agentTypeName: string, input: DataValue, phantomId: Uuid | undefined): string => {
    let phantomPostfix;
    if (phantomId) {
      phantomPostfix = `[${phantomId.highBits}-${phantomId.lowBits}]`;
    } else {
      phantomPostfix = '';
    }
    return `${agentTypeName}(${JSON.stringify(input)})${phantomPostfix}`;
  },
  parseAgentId(agentId: string): [string, DataValue, Uuid | undefined] {
    const match = agentId.match(/^(.*)\((.*)\)(\[(\d+)-(\d+)])?/);
    if (!match) {
      throw new Error(`Invalid agent ID: ${agentId}`);
    }
    const [, typeName, inputJson, maybePhantomId, hiBits, loBits] = match;
    const input = JSON.parse(inputJson);
    let phantomId: Uuid | undefined = undefined;
    if (maybePhantomId) {
      phantomId = {
        highBits: BigInt(hiBits),
        lowBits: BigInt(loBits),
      };
    }
    return [typeName, input, phantomId];
  },
  WasmRpc: vi
    .fn()
    .mockImplementation(
      (_agentTypeName: string, _constructor: DataValue, _phantomId: Uuid | undefined) => ({}),
    ),
}));

vi.mock('golem:core/types@1.5.0', () => ({
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

vi.mock('golem:quota/host@1.5.0', () => ({}));

(globalThis as any).currentAgentId = 'foo-agent(123)';

vi.mock('wasi:cli/environment@0.2.3', () => ({
  getEnvironment: () => [['GOLEM_AGENT_ID', (globalThis as any).currentAgentId]],
}));

await import('./agentsInit');
