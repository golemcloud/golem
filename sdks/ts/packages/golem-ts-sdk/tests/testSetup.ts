// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { DataValue, RegisteredAgentType, Uuid } from 'golem:agent/host';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src';
import { AgentId } from 'golem:rpc/types@0.2.2';

vi.mock('golem:agent/host', () => ({
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
    // Not a correct implementation, but good enough for some tests
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
}));

vi.mock('golem:rpc/types@0.2.2', () => ({
  WasmRpc: vi.fn().mockImplementation((_: AgentId) => ({})),
}));

(globalThis as unknown as { currentAgentId: string }).currentAgentId = 'foo-agent(123)';

vi.mock('wasi:cli/environment@0.2.3', () => ({
  getEnvironment: () => [
    ['GOLEM_AGENT_ID', (globalThis as unknown as { currentAgentId: string }).currentAgentId],
  ],
}));

await import('./agentsInit');
