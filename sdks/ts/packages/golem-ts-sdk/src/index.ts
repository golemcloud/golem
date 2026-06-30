// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

import { ResolvedAgent } from './internal/resolvedAgent';
import { AgentType, Principal } from 'golem:agent/common@2.0.0';
import { SchemaValueTree, uuidToString, parseUuid } from 'golem:core/types@2.0.0';
import type { Snapshot } from 'golem:api/host@1.5.0';
import type { InputStream } from 'wasi:io/streams@0.2.3';
import type { InvocationResult, Tool, ToolError, TypedSchemaValue } from 'golem:tool/common@0.1.0';
import { schemaValueFromWit } from './internal/schema-model';
import { createCustomError, isAgentError } from './internal/agentError';
import { AgentInitiatorRegistry } from './internal/registry/agentInitiatorRegistry';
import { getRawSelfAgentId } from './host/hostapi';
import { AgentInitiator } from './internal/agentInitiator';
import { setAgentId } from './internal/registry/agentId';
import { encodeMultipart, decodeMultipart } from './internal/multipart';
import { getAgentValidationError } from './decorators/agent';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';

export { BaseAgent } from './baseAgent';
export { Uuid } from './uuid';
export { ComponentId, AccountId, EnvironmentId } from './ids';
export { ParsedAgentId } from './agentId';
export { description } from './decorators/description';
export {
  agent,
  AgentDecoratorOptions,
  SnapshottingOption,
  clearAgentValidationError,
} from './decorators/agent';
export { prompt } from './decorators/prompt';
export { endpoint, EndpointDecoratorOptions } from './decorators/httpEndpoint';
export { readonly, ReadOnlyOptions, CachePolicyOption } from './decorators/readOnly';
export * from './agentClassName';
export * from './newTypes/textInput';
export * from './newTypes/binaryInput';
export * from './newTypes/multimodalAdvanced';
export { Principal } from './principal';
export { Client } from './baseAgent';
export { AgentClassName } from './agentClassName';
export { CancellationToken } from 'golem:agent/host@2.0.0';
export { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
export { TypescriptTypeRegistry } from './typescriptTypeRegistry';
export * from './webhook';
export * from './host/hostapi';
export * as oplog from './host/oplog';
export * from './host/guard';
export * from './host/quota';
export * from './host/retry';
export * from './host/result';
export * from './host/transaction';
export * from './host/checkpoint';
export { Config, Secret } from './agentConfig';
export { Path, Duration, Quantity } from './richTypes';

// Experimental fluent / config-object authoring surface (issue #3449), built on
// Standard Schema. Exported from the main entry so it is part of the bundle
// baked into `agent_guest.wasm` (and thus shares the runtime registries).
// Unstable; will eventually replace the `@agent()` decorator surface.
// Re-export the full fluent surface (defineAgent/method, markers `s`, clientFor,
// the typed host surfaces keyvalue/blobstore/websocket/rdbms, and the `http` helpers).
export * from './fluent';

let resolvedAgent: ResolvedAgent | undefined = undefined;
let initializationPrincipal: Principal | undefined = undefined;

interface GolemAgentGuest {
  initialize(agentTypeName: string, input: SchemaValueTree, principal: Principal): Promise<void>;
  discoverAgentTypes(): Promise<AgentType[]>;
  invoke(
    methodName: string,
    input: SchemaValueTree,
    principal: Principal,
  ): Promise<SchemaValueTree | undefined>;
  getDefinition(): Promise<AgentType>;
}

interface GolemToolGuest {
  discoverTools(): Promise<Tool[]>;
  getTool(name: string): Promise<Tool>;
  invoke(
    toolName: string,
    commandPath: string[],
    input: TypedSchemaValue,
    stdin: InputStream | undefined,
    principal: Principal,
  ): Promise<InvocationResult>;
}

interface SaveSnapshotGuest {
  save(): Promise<Snapshot>;
}

interface LoadSnapshotGuest {
  load(snapshot: Snapshot): Promise<void>;
}

async function initialize(
  agentTypeName: string,
  input: SchemaValueTree,
  principal: Principal,
): Promise<void> {
  // There shouldn't be a need to re-initialize an agent in a container.
  // If the input differs in a re-initialization, then that shouldn't be routed
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

  const initiateResult = await (initiator.initiateFromWit
    ? initiator.initiateFromWit(input, principal)
    : initiator.initiate(schemaValueFromWit(input), principal));

  if (initiateResult.tag === 'ok') {
    resolvedAgent = initiateResult.val;
    initializationPrincipal = principal;
  } else {
    throw initiateResult.val;
  }
}

async function invokeAgent(
  methodName: string,
  input: SchemaValueTree,
  principal: Principal,
): Promise<SchemaValueTree | undefined> {
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

async function discoverTools(): Promise<Tool[]> {
  return [];
}

async function getTool(name: string): Promise<Tool> {
  throw { tag: 'invalid-tool-name', val: name } satisfies ToolError;
}

async function invokeTool(
  toolName: string,
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _commandPath: string[],
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _input: TypedSchemaValue,
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _stdin: InputStream | undefined,
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  _principal: Principal,
): Promise<InvocationResult> {
  throw { tag: 'invalid-tool-name', val: toolName } satisfies ToolError;
}

async function discoverAgentTypes(): Promise<AgentType[]> {
  try {
    // Check if there were any validation errors during agent registration
    const validationError = getAgentValidationError();
    if (validationError) {
      // Don't return any agent types if there was a validation error
      throw createCustomError(validationError.message);
    }

    return AgentTypeRegistry.getRegisteredAgents();
  } catch (e) {
    // Have to throw RuntimeError, as the discover-agent-types WIT function returns result<list<agent-type>, RuntimeError>
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

function serializePrincipal(p: Principal): object {
  switch (p.tag) {
    case 'anonymous':
      return { tag: 'anonymous' };
    case 'agent':
      return {
        tag: 'agent',
        val: {
          componentId: uuidToString(p.val.agentId.componentId.uuid),
          agentId: p.val.agentId.agentId,
        },
      };
    case 'golem-user':
      return {
        tag: 'golem-user',
        val: { accountId: uuidToString(p.val.accountId.uuid) },
      };
    case 'oidc':
      return {
        tag: 'oidc',
        val: {
          sub: p.val.sub,
          issuer: p.val.issuer,
          email: p.val.email ?? null,
          name: p.val.name ?? null,
          emailVerified: p.val.emailVerified ?? null,
          givenName: p.val.givenName ?? null,
          familyName: p.val.familyName ?? null,
          picture: p.val.picture ?? null,
          preferredUsername: p.val.preferredUsername ?? null,
          claims: p.val.claims,
        },
      };
  }
}

function deserializePrincipal(obj: any): Principal {
  switch (obj.tag) {
    case 'anonymous':
      return { tag: 'anonymous' };
    case 'agent':
      return {
        tag: 'agent',
        val: {
          agentId: {
            componentId: { uuid: parseUuid(obj.val.componentId) },
            agentId: obj.val.agentId,
          },
        },
      };
    case 'golem-user':
      return {
        tag: 'golem-user',
        val: { accountId: { uuid: parseUuid(obj.val.accountId) } },
      };
    case 'oidc': {
      if (
        !obj.val ||
        typeof obj.val.sub !== 'string' ||
        typeof obj.val.issuer !== 'string' ||
        typeof obj.val.claims !== 'string'
      ) {
        throw new Error('Missing required fields (sub, issuer, claims) in oidc principal');
      }
      return {
        tag: 'oidc',
        val: {
          sub: obj.val.sub,
          issuer: obj.val.issuer,
          email: obj.val.email ?? undefined,
          name: obj.val.name ?? undefined,
          emailVerified: obj.val.emailVerified ?? undefined,
          givenName: obj.val.givenName ?? undefined,
          familyName: obj.val.familyName ?? undefined,
          picture: obj.val.picture ?? undefined,
          preferredUsername: obj.val.preferredUsername ?? undefined,
          claims: obj.val.claims,
        },
      };
    }
    default:
      throw new Error(`Unknown principal tag: ${obj.tag}`);
  }
}

async function save(): Promise<{ payload: Uint8Array; mimeType: string }> {
  if (!resolvedAgent) {
    throw new Error('Failed to save agent snapshot: agent is not initialized');
  }

  const { data: agentSnapshot, mimeType } = await resolvedAgent.saveSnapshot();
  const principal = initializationPrincipal ?? { tag: 'anonymous' };
  const serializedPrincipal = serializePrincipal(principal);

  if (mimeType.startsWith('multipart/mixed')) {
    // Multipart snapshot: the state JSON part already contains agent properties.
    // We need to inject version and principal into the state part.
    const boundaryMatch = mimeType.match(/boundary=([^\s;]+)/);
    if (!boundaryMatch) {
      throw new Error('multipart/mixed snapshot missing boundary parameter');
    }
    const boundary = boundaryMatch[1];
    const parts = decodeMultipart(agentSnapshot, boundary);

    const stateIdx = parts.findIndex((p) => p.name === 'state');
    if (stateIdx === -1) {
      throw new Error('multipart snapshot missing "state" part');
    }

    const stateJson = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
    const envelope = { version: 1, principal: serializedPrincipal, state: stateJson };
    parts[stateIdx] = {
      ...parts[stateIdx],
      body: new TextEncoder().encode(JSON.stringify(envelope)),
    };

    const { data, boundary: newBoundary } = encodeMultipart(parts);
    return {
      payload: data,
      mimeType: `multipart/mixed; boundary=${newBoundary}`,
    };
  } else if (mimeType === 'application/json') {
    // JSON snapshot: wrap in envelope { version, principal, state }
    const state = JSON.parse(new TextDecoder().decode(agentSnapshot));
    const envelope = { version: 1, principal: serializedPrincipal, state };
    return {
      payload: new TextEncoder().encode(JSON.stringify(envelope)),
      mimeType: 'application/json',
    };
  } else {
    // Binary snapshot: version-2 binary envelope with principal
    const principalJson = JSON.stringify(serializedPrincipal);
    const principalBytes = new TextEncoder().encode(principalJson);

    const totalLength = 1 + 4 + principalBytes.length + agentSnapshot.length;
    const fullSnapshot = new Uint8Array(totalLength);
    const view = new DataView(fullSnapshot.buffer);
    view.setUint8(0, 2); // version
    view.setUint32(1, principalBytes.length, false); // big-endian
    fullSnapshot.set(principalBytes, 5);
    fullSnapshot.set(agentSnapshot, 5 + principalBytes.length);

    return { payload: fullSnapshot, mimeType: 'application/octet-stream' };
  }
}

async function load(snapshot: { payload: Uint8Array; mimeType: string }): Promise<void> {
  const bytes = snapshot.payload;

  if (resolvedAgent) {
    throw `Agent is already initialized in this container`;
  }

  let agentSnapshot: Uint8Array;
  let agentSnapshotMimeType: string | undefined;
  let principal: Principal;

  if (snapshot.mimeType.startsWith('multipart/mixed')) {
    // Multipart snapshot: extract principal from the state JSON part
    const boundaryMatch = snapshot.mimeType.match(/boundary=([^\s;]+)/);
    if (!boundaryMatch) {
      throw 'multipart/mixed snapshot missing boundary parameter';
    }
    const boundary = boundaryMatch[1];
    const parts = decodeMultipart(bytes, boundary);

    const stateIdx = parts.findIndex((p) => p.name === 'state');
    if (stateIdx === -1) {
      throw 'multipart snapshot missing "state" part';
    }

    const envelope = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
    principal = envelope.principal
      ? deserializePrincipal(envelope.principal)
      : (initializationPrincipal ?? { tag: 'anonymous' });

    if (envelope.state === undefined) {
      throw `multipart state part missing 'state' field`;
    }

    // Replace the state part body with just the agent properties (strip version/principal)
    parts[stateIdx] = {
      ...parts[stateIdx],
      body: new TextEncoder().encode(JSON.stringify(envelope.state)),
    };

    // Re-encode the parts for loadSnapshot
    const { data: reencoded, boundary: newBoundary } = encodeMultipart(parts);
    agentSnapshot = reencoded;
    agentSnapshotMimeType = `multipart/mixed; boundary=${newBoundary}`;
  } else if (snapshot.mimeType === 'application/json') {
    // JSON snapshot: unwrap envelope { version, principal, state }
    const envelope = JSON.parse(new TextDecoder().decode(bytes));
    principal = envelope.principal
      ? deserializePrincipal(envelope.principal)
      : (initializationPrincipal ?? { tag: 'anonymous' });
    if (envelope.state === undefined) {
      throw `JSON snapshot missing 'state' field`;
    }
    agentSnapshot = new TextEncoder().encode(JSON.stringify(envelope.state));
    agentSnapshotMimeType = 'application/json';
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
      principal = deserializePrincipal(JSON.parse(new TextDecoder().decode(principalBytes)));
      agentSnapshot = bytes.slice(5 + principalLen);
    } else {
      throw `Unsupported snapshot version ${version}`;
    }
  }

  initializationPrincipal = principal;

  const [agentTypeName, agentParameters] = getRawSelfAgentId().parsed();

  const initiator = AgentInitiatorRegistry.lookup(agentTypeName);

  if (!initiator) {
    throw `Invalid agent'${agentTypeName}'. Valid agents are ${AgentInitiatorRegistry.agentTypeNames().join(', ')}`;
  }

  const initiateResult = await initiator.initiate(agentParameters, principal);

  if (initiateResult.tag === 'ok') {
    const agent = initiateResult.val;
    await agent.loadSnapshot(agentSnapshot, agentSnapshotMimeType);

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

export const golemAgent200Guest: GolemAgentGuest = {
  initialize,
  discoverAgentTypes,
  invoke: invokeAgent,
  getDefinition,
};

export const golemTool010Guest: GolemToolGuest = {
  discoverTools,
  getTool,
  invoke: invokeTool,
};

export const saveSnapshot: SaveSnapshotGuest = {
  save,
};

export const loadSnapshot: LoadSnapshotGuest = {
  load,
};
