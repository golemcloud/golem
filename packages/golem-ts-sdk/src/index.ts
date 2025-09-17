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

async function initialize(agentType: string, input: DataValue): Promise<void> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input (DataValue) differs in a re-initialization, then that shouldn't be routed
  // to this already-initialized container either.
  if (Option.isSome(resolvedAgent)) {
    throw createCustomError(`Agent is already initialized in this container`);
  }

  const initiator = AgentInitiatorRegistry.lookup(new AgentTypeName(agentType));

  if (Option.isNone(initiator)) {
    const entries = Array.from(AgentInitiatorRegistry.entries()).map(
      (entry) => entry[0].value,
    );

    throw createCustomError(
      `Invalid agent'${agentType}'. Valid agents are ${entries.join(', ')}`,
    );
  }

  const initiateResult = initiator.val.initiate(agentType, input);

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
  return AgentTypeRegistry.getRegisteredAgents();
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
  const agentParametersString = JSON.stringify(agentParameters);
  const agentParameterBytes = textEncoder.encode(agentParametersString);

  const agentSnapshot = await resolvedAgent.val.saveSnapshot();

  const agentTypeBytes = textEncoder.encode(agentType);
  const totalLength = 1 + 4 + 4 + agentTypeBytes.length + agentSnapshot.length;
  const fullSnapshot = new Uint8Array(totalLength);
  const view = new DataView(fullSnapshot.buffer);
  view.setUint8(0, 1); // version
  view.setUint32(1, agentTypeBytes.length);
  view.setUint32(1 + 4, agentParameterBytes.length);
  fullSnapshot.set(agentTypeBytes, 1 + 4 + 4);
  fullSnapshot.set(agentParameterBytes, 1 + 4 + 4 + agentTypeBytes.length);
  fullSnapshot.set(
    agentSnapshot,
    1 + 4 + 4 + agentTypeBytes.length + agentParameterBytes.length,
  );

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
  const agentTypeLength = view.getUint32(1);
  const agentParameterLength = view.getUint32(1 + 4);

  const agentType = textDecoder.decode(
    bytes.slice(1 + 4 + 4, 1 + 4 + 4 + agentTypeLength),
  );
  const agentParametersString = textDecoder.decode(
    bytes.slice(
      1 + 4 + 4 + agentTypeLength,
      1 + 4 + 4 + agentTypeLength + agentParameterLength,
    ),
  );
  const agentSnapshot = bytes.slice(
    1 + 4 + 4 + agentTypeLength + agentParameterLength,
  );

  const agentParameters: DataValue = JSON.parse(agentParametersString);

  const initiator = AgentInitiatorRegistry.lookup(new AgentTypeName(agentType));

  if (Option.isNone(initiator)) {
    const entries = Array.from(AgentInitiatorRegistry.entries()).map(
      (entry) => entry[0].value,
    );

    throw `Invalid agent'${agentType}'. Valid agents are ${entries.join(', ')}`;
  }

  const initiateResult = initiator.val.initiate(agentType, agentParameters);

  if (initiateResult.tag === 'ok') {
    const agent = initiateResult.val;
    await agent.loadSnapshot(agentSnapshot);

    resolvedAgent = Option.some(agent);
  } else {
    throw JSON.stringify(initiateResult.val);
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
