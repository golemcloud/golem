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
import { AgentClassName } from '../src';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { Result } from 'golem:rpc/types@0.2.2';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { AgentInitiator } from '../src/internal/agentInitiator';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

describe('AgentType look up', () => {
  it('AgentInitiatorRegistry should return the initiator when looking up by string representation of agentType', () => {
    const agentClassName = new AgentClassName('AssistantAgent');

    const FailingAgentInitiator: AgentInitiator = {
      initiate: (_constructorParams: DataValue): Result<ResolvedAgent, AgentError> => {
        return {
          tag: 'err',
          val: {
            tag: 'invalid-agent-id',
            val: 'unimplemented',
          },
        };
      },
    };

    AgentInitiatorRegistry.register(agentClassName, FailingAgentInitiator);

    const lookupResult = AgentInitiatorRegistry.lookup(agentClassName.value);

    expect(lookupResult).toEqual(FailingAgentInitiator);
  });

  it('AgentTypeRegistry should return the agent-type when looking up by string representation of agentClassName', () => {
    const agentClassName = new AgentClassName('AssistantAgent');
    const AgentTypeSample: AgentType = {
      typeName: agentClassName.value,
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
      mode: 'durable',
    };

    AgentTypeRegistry.register(agentClassName, AgentTypeSample);

    const agentType = AgentTypeRegistry.get(new AgentClassName('AssistantAgent'));

    expect(agentType).toEqual(AgentTypeSample);
  });

  it('AgentMethodMetadataRegistry should return method details when looking up by string representation of agentClassName', () => {
    const agentClassName = 'AssistantAgent';

    AgentMethodRegistry.setDescription(agentClassName, 'foo', 'sample desc');

    AgentMethodRegistry.setPrompt(agentClassName, 'foo', 'sample prompt');

    const lookupResult = AgentMethodRegistry.get(agentClassName);

    expect(lookupResult?.size).toEqual(1);

    const prompt = lookupResult?.get('foo')?.prompt;
    const description = lookupResult?.get('foo')?.description;

    expect(prompt).toEqual('sample prompt');
    expect(description).toEqual('sample desc');
  });

  it('AgentType should have ephemeral durability mode when set in options', () => {
    const agentClassName = new AgentClassName('EphemeralAgent');

    const agentType = AgentTypeRegistry.get(agentClassName);

    expect(agentType).toBeDefined;
    expect(agentType!.mode).toEqual('ephemeral');
  });

  it('AgentType should have durable durability mode by default', () => {
    const agentClassName = new AgentClassName('FooAgent');
    const agentType = AgentTypeRegistry.get(agentClassName);

    expect(agentType).toBeDefined;

    expect(agentType!.mode).toEqual('durable');
  });
});
