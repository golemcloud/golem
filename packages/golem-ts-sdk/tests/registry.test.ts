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
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { Result } from 'golem:rpc/types@0.2.2';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { AgentInitiator } from '../src/internal/agentInitiator';
import * as Option from '../src/newTypes/option';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentMethodMetadataRegistry } from '../src/internal/registry/agentMethodMetadataRegistry';

describe('AgentType look up', () => {
  it('AgentInitiatorRegistry should return the initiator when looking up by string representation of agentType', () => {
    const agentClassName = new AgentClassName('AssistantAgent');
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    const FailingAgentInitiator: AgentInitiator = {
      initiate: (
        _agentName: string,
        _constructorParams: DataValue,
      ): Result<ResolvedAgent, AgentError> => {
        return {
          tag: 'err',
          val: {
            tag: 'invalid-agent-id',
            val: 'unimplemented',
          },
        };
      },
    };

    AgentInitiatorRegistry.register(agentTypeName, FailingAgentInitiator);

    const lookupResult = AgentInitiatorRegistry.lookup(
      new AgentTypeName('assistant-agent'),
    );

    expect(lookupResult).toEqual(Option.some(FailingAgentInitiator));
  });

  it('AgentTypeRegistry should return the agent-type when looking up by string representation of agentClassName', () => {
    const agentClassName = new AgentClassName('AssistantAgent');
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);
    const AgentTypeSample: AgentType = {
      typeName: agentTypeName.value,
      description: 'An assistant agent',
      constructor: {
        name: 'foo',
        description: 'sample desc',
        promptHint: 'bar',
        inputSchema: {
          tag: 'tuple',
          val: [],
        },
      },
      methods: [],
      dependencies: [],
    };

    AgentTypeRegistry.register(agentClassName, AgentTypeSample);

    const lookupResult = AgentTypeRegistry.lookup(
      new AgentClassName('AssistantAgent'),
    );

    expect(lookupResult).toEqual(Option.some(AgentTypeSample));
  });

  it('AgentMethodMetadataRegistry should return method details when looking up by string representation of agentClassName', () => {
    const agentClassName = new AgentClassName('AssistantAgent');

    AgentMethodMetadataRegistry.setDescription(
      agentClassName,
      'foo',
      'sample desc',
    );

    AgentMethodMetadataRegistry.setPromptName(
      agentClassName,
      'foo',
      'sample prompt',
    );

    const lookupResult = AgentMethodMetadataRegistry.lookup(
      new AgentClassName('AssistantAgent'),
    );

    expect(lookupResult?.size).toEqual(1);

    const prompt = lookupResult?.get('foo')?.prompt;
    const description = lookupResult?.get('foo')?.description;

    expect(prompt).toEqual('sample prompt');
    expect(description).toEqual('sample desc');
  });
});
