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
import { AgentType, Principal, DataValue } from 'golem:agent/common';
import { createCustomError, isAgentError } from './internal/agentError';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from './host/hostapi';
import { AgentInitiator } from './internal/agentInitiator';

export { BaseAgent } from './baseAgent';
export { AgentId } from './agentId';
export { description } from './decorators/description';
export { agent, AgentDecoratorOptions } from './decorators/agent';
export { prompt } from './decorators/prompt';
export { endpoint, EndpointDecoratorOptions } from './decorators/httpEndpoint';

export * from './agentClassName';
export * from './newTypes/textInput';
export * from './newTypes/binaryInput';
export * from './newTypes/multimodalAdvanced';
export { Principal } from 'golem:agent/common';

export { Client } from './baseAgent';
export { AgentClassName } from './agentClassName';
export { TypescriptTypeRegistry } from './typescriptTypeRegistry';

export * from './host/hostapi';
export * from './host/guard';
export * from './host/result';
export * from './host/transaction';

let resolvedAgent: ResolvedAgent | undefined = undefined;
let initializationPrincipal: Principal | undefined = undefined;

async function initialize(
  agentTypeName: string,
  input: DataValue,
  principal: Principal,
): Promise<void> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input (DataValue) differs in a re-initialization, then that shouldn't be routed
  // to this already-initialized container either.
  if (resolvedAgent) {
    throw createCustomError(`Agent is already initialized in this container`);
  }

  const initiator: AgentInitiator | undefined = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw createCustomError(
      `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`,
    );
  }

  const initiateResult = initiator.initiate(input, principal);

  if (initiateResult.tag === 'ok') {
    resolvedAgent = initiateResult.val;
    initializationPrincipal = principal;
  } else {
    throw initiateResult.val;
  }
}

async function invoke(
  methodName: string,
  input: DataValue,
  principal: Principal,
): Promise<DataValue> {
  if (!resolvedAgent) {
    throw createCustomError(`Failed to invoke method ${methodName}: agent is not initialized`);
  }

  const result = await resolvedAgent.invoke(methodName, input, principal);

  if (result.tag === 'ok') {
    return result.val;
  } else {
    throw result.val;
  }
}

async function discoverAgentTypes(): Promise<bindings.guest.AgentType[]> {
  try {
    return AgentTypeRegistry.getRegisteredAgents();
  } catch (e) {
    // Have to throw AgentError, as the discover-agent-types WIT function returns result<list<agent-type>, AgentError>
    if (isAgentError(e)) {
      throw e;
    } else {
      throw createCustomError(String(e));
    }
  }
}

async function getDefinition(): Promise<AgentType> {
  if (!resolvedAgent) {
    throw new Error('Failed to get agent definition: agent is not initialized');
  }

  return resolvedAgent.getAgentType();
}

async function save(): Promise<Uint8Array> {
  if (!resolvedAgent) {
    throw new Error('Failed to save agent snapshot: agent is not initialized');
  }

  const agentSnapshot = await resolvedAgent.saveSnapshot();

  const totalLength = 1 + agentSnapshot.length;
  const fullSnapshot = new Uint8Array(totalLength);
  const view = new DataView(fullSnapshot.buffer);
  view.setUint8(0, 1); // version
  fullSnapshot.set(agentSnapshot, 1);

  return fullSnapshot;
}

async function load(bytes: Uint8Array): Promise<void> {
  if (resolvedAgent) {
    throw `Agent is already initialized in this container`;
  }

  const view = new DataView(bytes.buffer);
  const version = view.getUint8(0);
  if (version !== 1) {
    throw `Unsupported snapshot version ${version}`;
  }

  const agentSnapshot = bytes.slice(1);

  const [agentTypeName, agentParameters, _phantomId] = getRawSelfAgentId().parsed();

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  if (!initializationPrincipal) {
    throw `Failed to get agent definition: initializationPrincipal is not initialized`;
  }

  const initiateResult = initiator.initiate(agentParameters, initializationPrincipal);

  if (initiateResult.tag === 'ok') {
    const agent = initiateResult.val;
    await agent.loadSnapshot(agentSnapshot);

    resolvedAgent = agent;
  } else {
    // Throwing a String because the load WIT function returns result<_, string>
    let errorString = 'Failed to construct agent';
    try {
      errorString = JSON.stringify(initiateResult.val);
    } catch (e) {
      console.error('Failed to stringify agent construction error: ', e);
    }
    throw errorString;
  }
}

export const guest: typeof bindings.guest = {
  initialize,
  discoverAgentTypes,
  invoke,
  getDefinition,
};

export const saveSnapshot: typeof bindings.saveSnapshot = {
  save,
};

export const loadSnapshot: typeof bindings.loadSnapshot = {
  load,
};
