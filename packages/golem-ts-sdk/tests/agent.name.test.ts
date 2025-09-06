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

import { describe, expect, it } from 'vitest';
import { AgentTypeName } from '../src/newTypes/agentTypeName';
import { AgentClassName } from '../src';
import fc from 'fast-check';
import { agentClassNameArb } from './arbitraries';

describe('Conversion of TypeScript class names to valid kebab-case agent names', () => {
  it('should convert all type-script valid variations of `AssistantAgent` such as `_AssistantAgent$__1` to `assistant-agent`', () => {
    fc.assert(
      fc.property(agentClassNameArb, (agentClassName) => {
        const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);
        expect(agentTypeName.value).toEqual('assistant-agent');
      }),
    );
  });

  it('should convert `Assistant` to `assistant`', () => {
    const agentClassName = new AgentClassName('Assistant');
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    expect(agentTypeName.value).toEqual('assistant');
  });

  it('should preserve `assistant` as `assistant` itself', () => {
    const agentClassName = new AgentClassName('assistant');
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    expect(agentTypeName.value).toEqual('assistant');
  });

  it('should convert single letter `a` to `a', () => {
    const agentClassName = new AgentClassName('a');
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    expect(agentTypeName.value).toEqual('a');
  });
});
