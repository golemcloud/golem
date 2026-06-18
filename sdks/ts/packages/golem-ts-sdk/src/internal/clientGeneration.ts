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

import { ClassMetadata, Type, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { makeAgentId, WasmRpc, Datetime } from 'golem:agent/host@2.0.0';
import { AgentType, TypedAgentConfigValue } from 'golem:agent/common@2.0.0';
import { SchemaValueTree } from 'golem:core/types@2.0.0';
import { Uuid } from '../uuid';
import { AgentClassName } from '../agentClassName';
import * as Either from '../newTypes/either';
import { RemoteMethod } from '../baseAgent';
import { awaitPollable, throwIfAborted } from './pollableUtils';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import { RuntimeOutput, RuntimeParam } from './typeInfoInternal';
import { decodeOutput, encodeInputRecord } from './mapping/values/boundaryValue';
import { schemaValueFromWit, schemaValueToWit, typedSchemaValueToWit } from './schema-model';
import { mapTsTypeToResolvedGraph } from './mapping/types/resolvedMapper';
import { resolvedGraphToSchemaType } from './mapping/types/schemaType';
import { serializeGraph } from './mapping/values/schemaValue';
import { TypeScope } from './mapping/types/scope';
import { ParsedAgentId } from '../agentId';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
  configIncludedInArgs: boolean,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpcProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const constructedId = shared.constructWasmRpcParams(args, configIncludedInArgs);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, constructedId));
  };
}

export function getPhantomRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
  configIncludedInArgs: boolean,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }

  const shared = new WasmRpcProxyHandlerShared(metadata, agentClassName, agentType);

  return (finalPhantomId: Uuid, ...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const constructedId = shared.constructWasmRpcParams(args, configIncludedInArgs, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, constructedId));
  };
}

export function getNewPhantomRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
  configIncludedInArgs: boolean,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpcProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const finalPhantomId = Uuid.generate();
    const constructedId = shared.constructWasmRpcParams(args, configIncludedInArgs, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, constructedId));
  };
}

type CachedMethodInfo = {
  name: string;
  /** User-supplied parameters only (no `principal` / `config`). */
  params: RuntimeParam[];
  returnType: RuntimeOutput;
};

type WasmRpcParams = {
  agentTypeName: string;
  constructorTree: SchemaValueTree;
  phantomId: Uuid | undefined;
  agentIdString: string;
  agentConfigEntries: TypedAgentConfigValue[];
};

class WasmRpcProxyHandlerShared {
  readonly metadata: ClassMetadata;
  readonly agentClassName: AgentClassName;
  readonly agentType: AgentType;

  /** All constructor parameters (with names), in declaration order. */
  readonly constructorParams: RuntimeParam[];
  readonly cachedMethodInfo: Map<string, CachedMethodInfo> = new Map();

  constructor(metadata: ClassMetadata, agentClassName: AgentClassName, agentType: AgentType) {
    this.metadata = metadata;
    this.agentClassName = agentClassName;
    this.agentType = agentType;

    const constructorParamMeta =
      AgentConstructorParamRegistry.get(agentClassName.value) ?? new Map();

    this.constructorParams = [];
    for (const arg of metadata.constructorArgs) {
      const typeInfo = constructorParamMeta.get(arg.name)?.typeInfo;
      if (!typeInfo) {
        throw new Error(
          `No type information found for constructor parameter ${arg.name} in agent class ${agentClassName.value}`,
        );
      }
      this.constructorParams.push({ name: arg.name, type: typeInfo });
    }
  }

  hasMethod(methodName: string): boolean {
    return this.metadata.methods.has(methodName);
  }

  constructWasmRpcParams(
    args: any[],
    configIncludedInArgs: boolean,
    phantomId?: Uuid,
  ): WasmRpcParams {
    const userParams = this.constructorParams.filter(
      (p) => p.type.tag !== 'principal' && p.type.tag !== 'config',
    );
    const configParams = this.constructorParams.filter((p) => p.type.tag === 'config');

    const expectedArgs = configIncludedInArgs
      ? userParams.length + configParams.length
      : userParams.length;

    if (args.length > expectedArgs) {
      throw new Error(
        `Received more args than expected (got ${args.length}, expected ${expectedArgs})`,
      );
    }

    const userArgs = args.slice(0, userParams.length);
    const constructorInput = encodeInputRecord(userArgs, userParams);
    const constructorTree = schemaValueToWit(constructorInput);

    const agentConfigEntries: TypedAgentConfigValue[] = [];
    if (configIncludedInArgs) {
      const configArgs = args.slice(userParams.length);
      configParams.forEach((param, i) => {
        if (param.type.tag !== 'config') return;
        if (configArgs[i] === undefined) return;
        agentConfigEntries.push(
          ...serializeRpcConfigObject(configArgs[i], param.type.tsType.properties),
        );
      });
    }

    const agentTypeName = this.agentType.typeName;
    const agentIdString = makeAgentId(agentTypeName, constructorTree, phantomId);

    return {
      agentTypeName,
      constructorTree,
      phantomId,
      agentIdString,
      agentConfigEntries,
    };
  }

  getMethodInfo(methodName: string): CachedMethodInfo {
    const cachedInfo = this.cachedMethodInfo.get(methodName);
    if (cachedInfo) {
      return cachedInfo;
    } else {
      const methodSignature = this.metadata.methods.get(methodName);
      const methodParams = methodSignature?.methodParams;

      if (!methodParams) {
        throw new Error(
          `Unresolved method ${methodName} in RPC call. Make sure this method exists and is not private/protected`,
        );
      }

      const paramNames = Array.from(methodParams.keys());

      const paramTypeMap =
        AgentMethodParamRegistry.get(this.agentClassName.value)?.get(methodName) ?? new Map();

      const params: RuntimeParam[] = [];
      for (const paramName of paramNames) {
        const typeInfo = paramTypeMap.get(paramName)?.typeInfo;

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for parameter ${paramName} in method ${methodName} in agent class ${this.agentClassName.value}`,
          );
        }

        // Auto-injected `principal` parameters do not participate in the input record.
        if (typeInfo.tag === 'principal' || typeInfo.tag === 'config') {
          continue;
        }

        params.push({ name: paramName, type: typeInfo });
      }

      const returnType = AgentMethodRegistry.getReturnType(this.agentClassName.value, methodName);

      if (!returnType) {
        throw new Error(
          `Return type of method ${methodName} in agent class ${this.agentClassName.value} is not supported in remote calls`,
        );
      }

      const cachedInfo = {
        name: methodName,
        params,
        returnType,
      };
      this.cachedMethodInfo.set(methodName, cachedInfo);
      return cachedInfo;
    }
  }
}

class WasmRpcProxyHandler implements ProxyHandler<any> {
  private readonly shared: WasmRpcProxyHandlerShared;
  private readonly agentId: ParsedAgentId;
  private readonly wasmRpc: WasmRpc;

  private readonly methodProxyCache = new Map<string, RemoteMethod<any[], any>>();

  private readonly getIdMethod: () => ParsedAgentId = () => this.agentId;
  private readonly phantomIdMethod: () => Uuid | undefined = () => {
    const [, , phantomId] = this.agentId.parsed();
    return phantomId;
  };
  private readonly getAgentTypeMethod: () => AgentType = () => this.shared.agentType;

  constructor(shared: WasmRpcProxyHandlerShared, rpcParams: WasmRpcParams) {
    this.shared = shared;
    this.agentId = new ParsedAgentId(rpcParams.agentIdString);

    this.wasmRpc = new WasmRpc(
      rpcParams.agentTypeName,
      rpcParams.constructorTree,
      rpcParams.phantomId,
      rpcParams.agentConfigEntries,
    );
  }

  get(target: any, prop: string | symbol) {
    const val = target[prop];
    const propString = prop.toString();

    if (typeof val === 'function') {
      switch (propString) {
        case 'getId': {
          return this.getIdMethod;
        }
        case 'phantomId': {
          return this.phantomIdMethod;
        }
        case 'getAgentType': {
          return this.getAgentTypeMethod;
        }
        case 'loadSnapshot': {
          throw new Error('Cannot call loadSnapshot on a remote client');
        }
        case 'saveSnapshot': {
          throw new Error('Cannot call saveSnapshot on a remote client');
        }
      }
    }

    if (typeof prop === 'string' && this.shared.hasMethod(prop)) {
      const methodProxy = this.methodProxyCache.get(prop);
      if (methodProxy) {
        return methodProxy;
      } else {
        const methodProxy = this.createMethodProxy(prop);
        this.methodProxyCache.set(prop, methodProxy);
        return methodProxy;
      }
    }

    return val;
  }

  private createMethodProxy(prop: string): RemoteMethod<any[], any> {
    const methodInfo = this.shared.getMethodInfo(prop);
    const agentIdString = this.agentId.value;
    const wasmRpc = this.wasmRpc;

    async function invokeAndAwaitInternal(fnArgs: any[], signal?: AbortSignal) {
      throwIfAborted(signal);

      const inputTree = serializeArgs(methodInfo.params, fnArgs);

      const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(methodInfo.name, inputTree);

      const onAbort = signal
        ? () => {
            try {
              rpcResultFuture.cancel();
            } catch {
              // best-effort: ignore cancellation failures
            }
          }
        : undefined;

      if (signal && onAbort) {
        signal.addEventListener('abort', onAbort, { once: true });
      }

      try {
        const rpcResultPollable = rpcResultFuture.subscribe();

        await awaitPollable(rpcResultPollable, signal);

        const rpcResult = rpcResultFuture.get();

        if (!rpcResult) {
          throw new Error(
            `RPC to remote agent failed. Failed to invoke ${methodInfo.name} in agent ${agentIdString}`,
          );
        }

        if (rpcResult.tag === 'err') {
          throw new Error('Remote agent returned error result: ' + JSON.stringify(rpcResult.val));
        }

        return deserializeRpcResult(rpcResult.val, methodInfo.returnType);
      } finally {
        if (signal && onAbort) {
          signal.removeEventListener('abort', onAbort);
        }
      }
    }

    function invokeFireAndForget(...fnArgs: any[]) {
      const inputTree = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.invoke(methodInfo.name, inputTree);
    }

    function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
      const inputTree = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.scheduleInvocation(ts, methodInfo.name, inputTree);
    }

    function invokeScheduleCancelable(ts: Datetime, ...fnArgs: any[]) {
      const inputTree = serializeArgs(methodInfo.params, fnArgs);
      return wasmRpc.scheduleCancelableInvocation(ts, methodInfo.name, inputTree);
    }

    const methodFn: any = (...args: any[]) => invokeAndAwaitInternal(args);

    methodFn.abortable = (signal: AbortSignal, ...args: any[]) =>
      invokeAndAwaitInternal(args, signal);
    methodFn.trigger = (...args: any[]) => invokeFireAndForget(...args);
    methodFn.schedule = (ts: Datetime, ...args: any[]) => invokeSchedule(ts, ...args);
    methodFn.scheduleCancelable = (ts: Datetime, ...args: any[]) =>
      invokeScheduleCancelable(ts, ...args);

    return methodFn as RemoteMethod<any[], any>;
  }
}

/** Encode an ordered list of user-supplied method arguments into a `schema-value-tree`. */
function serializeArgs(userParams: RuntimeParam[], fnArgs: any[]): SchemaValueTree {
  return schemaValueToWit(encodeInputRecord(fnArgs, userParams));
}

function deserializeRpcResult(resultTree: SchemaValueTree | undefined, output: RuntimeOutput): any {
  const value = resultTree === undefined ? undefined : schemaValueFromWit(resultTree);
  return decodeOutput(value, output);
}

function serializeRpcConfigObject(
  rpcValue: unknown,
  configProperties: Type.ConfigProperty[],
): TypedAgentConfigValue[] {
  const result: TypedAgentConfigValue[] = [];

  if (rpcValue === null || typeof rpcValue !== 'object') {
    throw new Error(
      `Expected an object for config parameter \`${configProperties[0]?.path[0] ?? 'config'}\`, got ${typeof rpcValue}`,
    );
  }

  for (const prop of configProperties) {
    if (prop.secret) {
      continue;
    }

    let current: unknown = rpcValue;
    let missing = false;

    for (const key of prop.path) {
      if (current === null || typeof current !== 'object') {
        throw new Error(`Expected object while traversing config path ${prop.path.join('.')}`);
      }

      const record = current as Record<string, unknown>;
      current = record[key];

      if (current === undefined || current === null) {
        missing = true;
        break;
      }
    }

    if (missing) {
      continue;
    }

    const scope = TypeScope.object('config', prop.path.at(-1)!, prop.type.optional);
    const graph = Either.getOrThrowWith(
      mapTsTypeToResolvedGraph(prop.type, scope),
      (err) =>
        new Error(
          `Failed to construct schema for rpc config property \`${prop.path.join('.')}\`: ${err}`,
        ),
    );

    const value = serializeGraph(current, graph);
    const schemaGraph = resolvedGraphToSchemaType(graph).graph;

    result.push({
      path: prop.path,
      value: typedSchemaValueToWit({ graph: schemaGraph, value }),
    });
  }

  return result;
}
