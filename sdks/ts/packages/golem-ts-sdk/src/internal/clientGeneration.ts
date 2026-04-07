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
import * as WitValue from './mapping/values/WitValue';
import { makeAgentId, WasmRpc, Datetime } from 'golem:agent/host@1.5.0';
import { Uuid } from '../uuid';
import { AgentClassName } from '../agentClassName';
import * as WitType from './mapping/types/WitType';
import * as Either from '../newTypes/either';
import {
  AgentType,
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
  TypedAgentConfigValue,
} from 'golem:agent/common@1.5.0';
import { RemoteMethod } from '../baseAgent';
import { awaitPollable, throwIfAborted } from './pollableUtils';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import {
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
} from './mapping/values/serializer';
import { TypeInfoInternal } from './typeInfoInternal';
import { deserializeDataValue, serializeToDataValue } from './mapping/values/dataValue';
import { ValueAndType } from '../host/hostapi';
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

type CachedParamInfo = {
  name: string;
  type: TypeInfoInternal;
};

type CachedMethodInfo = {
  name: string;
  params: CachedParamInfo[];
  returnType: TypeInfoInternal;
};

type WasmRpcParams = {
  agentTypeName: string;
  constructorDataValue: DataValue;
  phantomId: Uuid | undefined;
  agentIdString: string;
  agentConfigEntries: TypedAgentConfigValue[];
};

class WasmRpcProxyHandlerShared {
  readonly metadata: ClassMetadata;
  readonly agentClassName: AgentClassName;
  readonly agentType: AgentType;

  readonly constructorParamTypes: TypeInfoInternal[];
  readonly cachedMethodInfo: Map<string, CachedMethodInfo> = new Map();

  constructor(metadata: ClassMetadata, agentClassName: AgentClassName, agentType: AgentType) {
    this.metadata = metadata;
    this.agentClassName = agentClassName;
    this.agentType = agentType;

    const constructorParamMeta =
      AgentConstructorParamRegistry.get(agentClassName.value) ?? new Map();

    this.constructorParamTypes = [];
    for (const arg of metadata.constructorArgs) {
      const typeInfo = constructorParamMeta.get(arg.name)?.typeInfo;
      if (!typeInfo) {
        throw new Error(
          `No type information found for constructor parameter ${arg.name} in agent class ${agentClassName.value}`,
        );
      }
      this.constructorParamTypes.push(typeInfo);
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
    let constructorDataValue: DataValue;
    const agentConfigEntries: TypedAgentConfigValue[] = [];

    let orderedConstructorParamsTypes;
    if (configIncludedInArgs) {
      orderedConstructorParamsTypes = [
        ...this.constructorParamTypes.filter((cp) => cp.tag !== 'config'),
        ...this.constructorParamTypes.filter((cp) => cp.tag === 'config'),
      ];
    } else {
      orderedConstructorParamsTypes = this.constructorParamTypes.filter(
        (cp) => cp.tag !== 'config',
      );
    }

    if (args.length === 1 && orderedConstructorParamsTypes[0].tag === 'multimodal') {
      constructorDataValue = serializeToDataValue(args[0], this.constructorParamTypes[0]);
    } else {
      const elementValues: ElementValue[] = [];

      for (const [index, arg] of args.entries()) {
        if (index >= orderedConstructorParamsTypes.length) {
          throw new Error('Received more args than expected');
        }
        let typeInfoInternal = orderedConstructorParamsTypes[index];

        switch (typeInfoInternal.tag) {
          case 'analysed':
            const witValue = WitValue.fromTsValueDefault(arg, typeInfoInternal.val);
            const elementValue: ElementValue = {
              tag: 'component-model',
              val: witValue,
            };
            elementValues.push(elementValue);
            break;
          case 'unstructured-text': {
            const textReference: TextReference = serializeTsValueToTextReference(arg);

            const elementValue: ElementValue = {
              tag: 'unstructured-text',
              val: textReference,
            };

            elementValues.push(elementValue);
            break;
          }
          case 'unstructured-binary':
            const binaryReference: BinaryReference = serializeTsValueToBinaryReference(arg);

            const elementValueBinary: ElementValue = {
              tag: 'unstructured-binary',
              val: binaryReference,
            };

            elementValues.push(elementValueBinary);
            break;
          case 'multimodal':
            throw new Error('Multimodal constructor parameters are not supported in remote calls');
          case 'config': {
            agentConfigEntries.push(
              ...serializeRpcConfigObject(arg, typeInfoInternal.tsType.properties),
            );
          }
        }
      }

      constructorDataValue = {
        tag: 'tuple',
        val: elementValues,
      };
    }

    const agentIdString = makeAgentId(this.agentClassName.value, constructorDataValue, phantomId);

    return {
      agentTypeName: this.agentClassName.value,
      constructorDataValue,
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

      const params = [];
      for (const paramName of paramNames) {
        const typeInfo = paramTypeMap.get(paramName)?.typeInfo;

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for parameter ${paramNames} in method ${methodName} in agent class ${this.agentClassName.value}`,
          );
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
      rpcParams.constructorDataValue,
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

      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);

      const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(methodInfo.name, inputDataValue);

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

        const resultDataValue =
          rpcResult.tag === 'err'
            ? (() => {
                throw new Error(
                  'Remote agent returned error result: ' + JSON.stringify(rpcResult.val),
                );
              })()
            : rpcResult.val;

        return deserializeRpcResult(resultDataValue, methodInfo.returnType);
      } finally {
        if (signal && onAbort) {
          signal.removeEventListener('abort', onAbort);
        }
      }
    }

    function invokeFireAndForget(...fnArgs: any[]) {
      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.invoke(methodInfo.name, inputDataValue);
    }

    function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.scheduleInvocation(ts, methodInfo.name, inputDataValue);
    }

    const methodFn: any = (...args: any[]) => invokeAndAwaitInternal(args);

    methodFn.abortable = (signal: AbortSignal, ...args: any[]) =>
      invokeAndAwaitInternal(args, signal);
    methodFn.trigger = (...args: any[]) => invokeFireAndForget(...args);
    methodFn.schedule = (ts: Datetime, ...args: any[]) => invokeSchedule(ts, ...args);

    return methodFn as RemoteMethod<any[], any>;
  }
}

function serializeArgs(params: CachedParamInfo[], fnArgs: any[]): DataValue {
  const elementValues: ElementValue[] = [];
  for (const [index, fnArg] of fnArgs.entries()) {
    const param = params[index];

    switch (param.type.tag) {
      case 'analysed': {
        const witValue = WitValue.fromTsValueDefault(fnArg, param.type.val);
        elementValues.push({ tag: 'component-model', val: witValue });
        break;
      }
      case 'unstructured-text': {
        const textRef: TextReference = serializeTsValueToTextReference(fnArg);
        elementValues.push({ tag: 'unstructured-text', val: textRef });
        break;
      }
      case 'unstructured-binary': {
        const binRef: BinaryReference = serializeTsValueToBinaryReference(fnArg);
        elementValues.push({ tag: 'unstructured-binary', val: binRef });
        break;
      }
      case 'principal':
        throw new Error(
          'Internal error: Value of `Principal` should not be serialized at any point during RPC call',
        );
      case 'config':
        throw new Error(
          'Internal error: Value of `Config` should not be serialized at any point during RPC call',
        );
      case 'multimodal': {
        // For a multimodal param, the serialized DataValue is itself the result;
        // we wrap it as a single tuple with the multimodal elements
        const multimodalDv = serializeToDataValue(fnArg, param.type);
        if (multimodalDv.tag === 'multimodal') {
          // Each multimodal element becomes part of the overall DataValue
          // But since params are tuple-based, we need to wrap multimodal as a single element
          // The server expects each param as an ElementValue in the tuple
          // For multimodal, we serialize the whole thing as a component-model WitValue
          for (const [, ev] of multimodalDv.val) {
            elementValues.push(ev);
          }
        } else {
          for (const ev of multimodalDv.val) {
            elementValues.push(ev);
          }
        }
        break;
      }
    }
  }
  return { tag: 'tuple', val: elementValues };
}

function deserializeRpcResult(resultDataValue: DataValue, typeInfoInternal: TypeInfoInternal): any {
  return deserializeDataValue(
    resultDataValue,
    [
      {
        name: 'returnValue',
        type: typeInfoInternal,
      },
    ],
    { tag: 'anonymous' },
  )[0];
}

function serializeRpcConfigObject(
  rpcValue: unknown,
  configProperties: Type.ConfigProperty[],
): TypedAgentConfigValue[] {
  const result: TypedAgentConfigValue[] = [];

  if (rpcValue === null || typeof rpcValue !== 'object') {
    throw new Error('rpcValue must be an object');
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

    const expectedType = prop.type;

    const [witType, analysedType] = Either.getOrThrowWith(
      WitType.fromTsType(expectedType, undefined),
      (err) => new Error(`Failed to construct analysed type for rpc agent config: ${err}`),
    );

    const witValue = WitValue.fromTsValueDefault(current, analysedType);

    const valueAndType: ValueAndType = {
      typ: witType,
      value: witValue,
    };

    result.push({
      path: prop.path,
      value: valueAndType,
    });
  }

  return result;
}
