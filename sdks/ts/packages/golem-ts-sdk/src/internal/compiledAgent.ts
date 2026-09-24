// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { AgentType, Principal } from 'golem:agent/common@2.0.0';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import { AgentClassName } from '../agentClassName';
import { registerAgentInitiator, type AgentRuntime } from '../runtime';
import { sdkPrincipalFromHost } from '../principal';
import { AgentTypeRegistry } from './registry/agentTypeRegistry';
import {
  readConcrete,
  readConcreteAsync,
  writeConcrete,
  writeConcreteAsync,
  type ConcreteCodec,
} from './tool/compiled';
import type { StandardSchemaV1 } from '../schema/standardSchema';
import { ParsedAgentId, bindAgentClient } from '../agentId';
import { Uuid } from '../uuid';
import { makeAgentId, type Datetime } from 'golem:agent/host@2.0.0';
import { resolveWireRemoteAgent, RemoteOutputError } from '../bridge/agent';
import { compiledConfig, type ConfigNode } from './compiledConfig';

export const concretePrincipal = { fromHost: sdkPrincipalFromHost };

interface Input {
  codec: ConcreteCodec;
  principals: string[];
  hasInput: boolean;
  hasCallerInput: boolean;
}

function readInput(
  input: Input,
  tree: SchemaValueTree,
  principal: Principal,
): Record<string, unknown> {
  const record = readConcrete(input.codec, tree) as Record<string, unknown>;
  for (const name of input.principals) record[name] = sdkPrincipalFromHost(principal);
  return record;
}

/** Compiler-emitted agent contract; lifecycle and snapshot policy remain shared with dynamic agents. */
export function compiledAgent(
  descriptor: AgentType,
  id: Input,
  methods: Array<{ name: string; input: Input; output?: ConcreteCodec }>,
  snapshotStateSchema: StandardSchemaV1 | undefined,
  snapshotting: boolean,
  config: ConfigNode[],
) {
  const name = descriptor.typeName;
  const configuration = compiledConfig(config);
  const reg: AgentRuntime = {
    name,
    className: new AgentClassName(name),
    agentType: descriptor,
    readId: (tree, principal) => readInput(id, tree, principal),
    runtimeMethods: new Map(
      methods.map((method) => [
        method.name,
        {
          hasInput: method.input.hasInput,
          read: async (tree: SchemaValueTree, principal: Principal) => {
            const record = (await readConcreteAsync(method.input.codec, tree)) as Record<
              string,
              unknown
            >;
            for (const name of method.input.principals)
              record[name] = sdkPrincipalFromHost(principal);
            return record;
          },
          write: async (value: unknown) =>
            method.output ? writeConcreteAsync(method.output, value) : undefined,
        },
      ]),
    ),
    configAccessor: configuration.accessor,
    snapshotStateSchema,
  };
  let registered = false;
  try {
    AgentTypeRegistry.register(reg.className, descriptor);
    registered = true;
  } catch (error) {
    AgentTypeRegistry.recordRegistrationError(name, `Definition failed: ${String(error)}`);
  }
  const mode = descriptor.mode;
  const agentId = (value: unknown, phantom?: Uuid) =>
    new ParsedAgentId(makeAgentId(name, writeConcrete(id.codec, value), phantom));
  const bind = (tree: SchemaValueTree, phantom?: Uuid, overrides?: Record<string, unknown>) => {
    const remote = resolveWireRemoteAgent(
      name,
      tree,
      phantom,
      configuration.overrides(overrides),
      mode,
    );
    return Object.fromEntries(
      methods.map((method) => {
        const invoke = async (value: unknown, signal?: AbortSignal) => {
          const result = await remote.invokeAndAwaitWithMetadata(
            method.name,
            await writeConcreteAsync(method.input.codec, value),
            signal,
          );
          let output;
          if (method.output) {
            if (result.value === undefined)
              throw new RemoteOutputError(`Remote agent ${name}.${method.name} returned no value`);
            try {
              output = await readConcreteAsync(method.output, result.value);
            } catch (cause) {
              throw new RemoteOutputError(
                `Remote agent ${name}.${method.name} returned an invalid output`,
                { cause },
              );
            }
          }
          return mode === 'ephemeral' ? { metadata: result.metadata, value: output } : output;
        };
        const call = method.input.hasCallerInput
          ? (value: unknown, options?: { signal?: AbortSignal }) => invoke(value, options?.signal)
          : (options?: { signal?: AbortSignal }) => invoke({}, options?.signal);
        return [
          method.name,
          Object.assign(call, {
            trigger(value: unknown = {}) {
              const metadata = remote.invokeWithMetadata(
                method.name,
                writeConcrete(method.input.codec, value),
              );
              return mode === 'ephemeral' ? metadata : undefined;
            },
            schedule(at: Datetime, value: unknown = {}) {
              const receipt = remote.scheduleCancelableWithMetadata(
                at,
                method.name,
                writeConcrete(method.input.codec, value),
              );
              return mode === 'ephemeral' ? receipt : receipt.cancellationToken;
            },
          }),
        ];
      }),
    );
  };
  const createClient = (value: unknown, phantom?: Uuid, overrides?: Record<string, unknown>) =>
    bind(writeConcrete(id.codec, value), phantom, overrides);
  const client = {
    ...(mode === 'durable'
      ? {
          get: (value: unknown, overrides?: Record<string, unknown>) =>
            createClient(value, undefined, overrides),
        }
      : {}),
    getPhantom: createClient,
    newPhantom(value: unknown, overrides?: Record<string, unknown>) {
      if (mode === 'ephemeral') return createClient(value, undefined, overrides);
      const phantomId = Uuid.generate();
      return {
        client: createClient(value, phantomId, overrides),
        agentId: agentId(value, phantomId),
        phantomId,
      };
    },
  };
  let implemented = false;
  return {
    name,
    client,
    agentId,
    [bindAgentClient](identity: ParsedAgentId) {
      const [type, tree, phantom] = identity.parsedWire();
      if (type !== name) throw new TypeError(`Agent client '${name}' cannot bind '${type}'`);
      if (mode === 'ephemeral') throw new TypeError('Cannot bind an existing ephemeral agent');
      return bind(tree, phantom);
    },
    implement(impl: Parameters<typeof registerAgentInitiator>[1]) {
      try {
        if (implemented)
          throw new Error('implement() was called more than once for this definition');
        implemented = true;
        if (!registered) return { name };
        if (
          impl.snapshot &&
          (typeof impl.snapshot.load !== 'function' ||
            (typeof impl.snapshot.save !== 'function' &&
              !(snapshotStateSchema && impl.snapshot.save === undefined)))
        )
          throw new Error('custom snapshotting requires both snapshot.save and snapshot.load');
        if (snapshotting && !snapshotStateSchema && typeof impl.snapshot?.save !== 'function')
          throw new Error(
            'snapshotting without a state schema requires snapshot.save and snapshot.load',
          );
        registerAgentInitiator(reg, impl);
      } catch (error) {
        AgentTypeRegistry.recordRegistrationError(name, `Implementation failed: ${String(error)}`);
      }
      return { name };
    },
  };
}
