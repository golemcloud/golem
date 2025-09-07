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

import type * as bindings from 'agent-guest';
import { ResolvedAgent } from './internal/resolvedAgent';
import { Result } from 'golem:rpc/types@0.2.2';
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { createCustomError } from './internal/agentError';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import * as Option from './newTypes/option';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { AgentTypeName } from './newTypes/agentTypeName';

export { BaseAgent } from './baseAgent';
export { AgentId } from './agentId';
export { prompt, description, agent } from './decorators';
export * from './newTypes/either';
export * from './newTypes/agentClassName';
export * from './newTypes/textInput';

export { AgentClassName } from './newTypes/agentClassName';
export { TypescriptTypeRegistry } from './typescriptTypeRegistry';

export * from './host/hostapi';
export * from './host/guard';
export * from './host/result';
export * from './host/transaction';

let resolvedAgent: Option.Option<ResolvedAgent> = Option.none();

const UninitiatedAgentErrorMessage: string = 'Agent is not initialized';

const UninitializedAgentError: AgentError = createCustomError(
  UninitiatedAgentErrorMessage,
);

// An error can happen if the user agent is not composed (which will initialize the agent with precompiled wasm)
function getResolvedAgentOrThrow(
  resolvedAgent: Option.Option<ResolvedAgent>,
): ResolvedAgent {
  return Option.getOrThrowWith(
    resolvedAgent,
    () => new Error(UninitiatedAgentErrorMessage),
  );
}

async function initialize(
  agentType: string,
  input: DataValue,
): Promise<Result<void, AgentError>> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input (DataValue) differs in a re-initialization, then that shouldn't be routed
  // to this already-initialized container either.
  if (Option.isSome(resolvedAgent)) {
    return {
      tag: 'err',
      val: createCustomError(`Agent is already initialized in this container`),
    };
  }

  const initiator = AgentInitiatorRegistry.lookup(new AgentTypeName(agentType));

  if (Option.isNone(initiator)) {
    const entries = Array.from(AgentInitiatorRegistry.entries()).map(
      (entry) => entry[0].value,
    );

    return {
      tag: 'err',
      val: createCustomError(
        `Invalid agent'${agentType}'. Valid agents are ${entries.join(', ')}`,
      ),
    };
  }

  const initiateResult = initiator.val.initiate(agentType, input);

  if (initiateResult.tag === 'ok') {
    resolvedAgent = Option.some(initiateResult.val);

    return {
      tag: 'ok',
      val: undefined,
    };
  }

  return {
    tag: 'err',
    val: initiateResult.val,
  };
}

async function invoke(
  methodName: string,
  input: DataValue,
): Promise<Result<DataValue, AgentError>> {
  if (Option.isNone(resolvedAgent)) {
    return {
      tag: 'err',
      val: UninitializedAgentError,
    };
  }

  return resolvedAgent.val.invoke(methodName, input);
}

async function discoverAgentTypes(): Promise<bindings.guest.AgentType[]> {
  return AgentTypeRegistry.getRegisteredAgents();
}

async function getDefinition(): Promise<AgentType> {
  return getResolvedAgentOrThrow(resolvedAgent).getDefinition();
}

export const guest: typeof bindings.guest = {
  initialize,
  discoverAgentTypes,
  invoke,
  getDefinition,
};
