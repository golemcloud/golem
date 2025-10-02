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
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { createCustomError, isAgentError } from './internal/agentError';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import * as Option from './newTypes/option';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { makeAgentId, parseAgentId } from 'golem:agent/host';

export { BaseAgent } from './baseAgent';
export { AgentId } from './agentId';
export {
  prompt,
  description,
  agent,
  languageCodes,
  mimeTypes,
  multimodal,
} from './decorators';
export * from './newTypes/either';
export * from './newTypes/agentClassName';
export * from './newTypes/textInput';
export * from './newTypes/binaryInput';

export { WithRemoteMethods } from './baseAgent';
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
  agentTypeName: string,
  input: DataValue,
): Promise<void> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input (DataValue) differs in a re-initialization, then that shouldn't be routed
  // to this already-initialized container either.
  if (Option.isSome(resolvedAgent)) {
    throw createCustomError(`Agent is already initialized in this container`);
  }

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (Option.isNone(initiator)) {
    throw createCustomError(
      `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`,
    );
  }

  const initiateResult = initiator.val.initiate(input);

  if (initiateResult.tag === 'ok') {
    resolvedAgent = Option.some(initiateResult.val);
  } else {
    throw initiateResult.val;
  }
}

async function invoke(
  methodName: string,
  input: DataValue,
): Promise<DataValue> {
  if (Option.isNone(resolvedAgent)) {
    throw UninitializedAgentError;
  }

  const result = await resolvedAgent.val.invoke(methodName, input);
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
  return getResolvedAgentOrThrow(resolvedAgent).getDefinition();
}

async function save(): Promise<Uint8Array> {
  if (Option.isNone(resolvedAgent)) {
    throw UninitializedAgentError;
  }

  const textEncoder = new TextEncoder();

  const agentType = resolvedAgent.val.getDefinition().typeName;
  const agentParameters = resolvedAgent.val.getParameters();

  const agentIdString = makeAgentId(agentType, agentParameters);
  const agentIdBytes = textEncoder.encode(agentIdString);

  const agentSnapshot = await resolvedAgent.val.saveSnapshot();

  const totalLength = 1 + 4 + agentIdBytes.length + agentSnapshot.length;
  const fullSnapshot = new Uint8Array(totalLength);
  const view = new DataView(fullSnapshot.buffer);
  view.setUint8(0, 1); // version
  view.setUint32(1, agentIdBytes.length);
  fullSnapshot.set(agentIdBytes, 1 + 4);
  fullSnapshot.set(agentSnapshot, 1 + 4 + agentIdBytes.length);

  return fullSnapshot;
}

async function load(bytes: Uint8Array): Promise<void> {
  if (Option.isSome(resolvedAgent)) {
    throw `Agent is already initialized in this container`;
  }

  const textDecoder = new TextDecoder();

  const view = new DataView(bytes.buffer);
  const version = view.getUint8(0);
  if (version !== 1) {
    throw `Unsupported snapshot version ${version}`;
  }
  const agentIdLength = view.getUint32(1);

  const agentId = textDecoder.decode(bytes.slice(1 + 4, 1 + 4 + agentIdLength));
  const agentSnapshot = bytes.slice(1 + 4 + agentIdLength);

  const [agentTypeName, agentParameters] = parseAgentId(agentId);

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (Option.isNone(initiator)) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  const initiateResult = initiator.val.initiate(agentParameters);

  if (initiateResult.tag === 'ok') {
    const agent = initiateResult.val;
    await agent.loadSnapshot(agentSnapshot);

    resolvedAgent = Option.some(agent);
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
