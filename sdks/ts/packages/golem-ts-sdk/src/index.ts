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
import { setAgentId } from './internal/registry/agentId';

export { BaseAgent } from './baseAgent';
export { AgentId } from './agentId';
export { description } from './decorators/description';
export { agent, AgentDecoratorOptions, SnapshottingOption } from './decorators/agent';
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

export * from './webhook';

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

  setAgentId(getRawSelfAgentId());

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

async function save(): Promise<{ data: Uint8Array; mimeType: string }> {
  if (!resolvedAgent) {
    throw new Error('Failed to save agent snapshot: agent is not initialized');
  }

  const { data: agentSnapshot, mimeType } = await resolvedAgent.saveSnapshot();
  const principal = initializationPrincipal ?? { tag: 'anonymous' };

  if (mimeType === 'application/json') {
    // JSON snapshot: wrap in envelope { version, principal, state }
    const state = JSON.parse(new TextDecoder().decode(agentSnapshot));
    const envelope = { version: 1, principal, state };
    return {
      data: new TextEncoder().encode(JSON.stringify(envelope)),
      mimeType: 'application/json',
    };
  } else {
    // Binary snapshot: version-2 binary envelope with principal
    const principalJson = JSON.stringify(principal);
    const principalBytes = new TextEncoder().encode(principalJson);

    const totalLength = 1 + 4 + principalBytes.length + agentSnapshot.length;
    const fullSnapshot = new Uint8Array(totalLength);
    const view = new DataView(fullSnapshot.buffer);
    view.setUint8(0, 2); // version
    view.setUint32(1, principalBytes.length, false); // big-endian
    fullSnapshot.set(principalBytes, 5);
    fullSnapshot.set(agentSnapshot, 5 + principalBytes.length);

    return { data: fullSnapshot, mimeType: 'application/octet-stream' };
  }
}

async function load(snapshot: { data: Uint8Array; mimeType: string }): Promise<void> {
  const bytes = snapshot.data;

  if (resolvedAgent) {
    throw `Agent is already initialized in this container`;
  }

  let agentSnapshot: Uint8Array;
  let principal: Principal;

  if (snapshot.mimeType === 'application/json') {
    // JSON snapshot: unwrap envelope { version, principal, state }
    const envelope = JSON.parse(new TextDecoder().decode(bytes));
    principal = envelope.principal ?? initializationPrincipal ?? { tag: 'anonymous' };
    if (envelope.state === undefined) {
      throw `JSON snapshot missing 'state' field`;
    }
    agentSnapshot = new TextEncoder().encode(JSON.stringify(envelope.state));
  } else {
    // Custom binary snapshot with version envelope
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const version = view.getUint8(0);

    if (version === 1) {
      agentSnapshot = bytes.slice(1);
      principal = initializationPrincipal ?? { tag: 'anonymous' };
    } else if (version === 2) {
      const principalLen = view.getUint32(1, false); // big-endian
      const principalBytes = bytes.slice(5, 5 + principalLen);
      principal = JSON.parse(new TextDecoder().decode(principalBytes)) as Principal;
      agentSnapshot = bytes.slice(5 + principalLen);
    } else {
      throw `Unsupported snapshot version ${version}`;
    }
  }

  initializationPrincipal = principal;

  const [agentTypeName, agentParameters, _phantomId] = getRawSelfAgentId().parsed();

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  const initiateResult = initiator.initiate(agentParameters, principal);

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
