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

import * as Option from '../src/newTypes/option';
import { DataValue, RegisteredAgentType, Uuid } from 'golem:agent/host';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src';
import { AgentId } from 'golem:rpc/types@0.2.2';

vi.mock('golem:agent/host', () => ({
  getAgentType: (agentTypeName: string): RegisteredAgentType | undefined => {
    if (agentTypeName == 'FooAgent') {
      return {
        agentType: Option.getOrThrowWith(
          AgentTypeRegistry.get(new AgentClassName('FooAgent')),
          () => new Error('Missing FooAgent'),
        ),
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
  makeAgentId: (
    agentTypeName: string,
    input: DataValue,
    phantomId: Uuid | undefined,
  ): string => {
    // Not a correct implementation, but good enough for some tests
    let phantomPostfix;
    if (phantomId) {
      phantomPostfix = `[$phantomId]`;
    } else {
      phantomPostfix = '';
    }
    return `${agentTypeName}(${JSON.stringify(input)})$phantomPostfix`;
  },
}));

vi.mock('golem:rpc/types@0.2.2', () => ({
  WasmRpc: vi.fn().mockImplementation((_: AgentId) => ({})),
}));

await import('./agentsInit');
