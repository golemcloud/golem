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

import { ClassMetadata, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import * as WitValue from './mapping/values/WitValue';
import * as Either from '../newTypes/either';
import {
  getAgentType,
  makeAgentId,
  RegisteredAgentType,
  Uuid,
  WasmRpc,
  Datetime,
} from 'golem:agent/host@1.5.0';
import { AgentClassName } from '../agentClassName';
import {
  AgentType,
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
} from 'golem:agent/common@1.5.0';
import { RemoteMethod } from '../baseAgent';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import {
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
} from './mapping/values/serializer';
import { TypeInfoInternal } from './typeInfoInternal';
import {
  deserializeDataValue,
  ParameterDetail,
  serializeToDataValue,
} from './mapping/values/dataValue';
import { randomUuid } from '../host/hostapi';
import { AgentId } from '../agentId';
import * as util from 'node:util';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const constructedId = shared.constructAgentId(args);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, constructedId));
  };
}

export function getPhantomRemoteClient<T extends new (phantomId: Uuid, ...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }

  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (finalPhantomId: Uuid, ...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const constructedId = shared.constructAgentId(args, finalPhantomId);

    return new Proxy(instance, new WasmRpcProxyHandler(shared, constructedId));
  };
}

export function getNewPhantomRemoteClient<T extends new (...args: any[]) => any>(
  agentClassName: AgentClassName,
  agentType: AgentType,
  ctor: T,
) {
  const metadata = TypeMetadata.get(ctor.name);

  if (!metadata) {
    throw new Error(
      `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
    );
  }
  const shared = new WasmRpxProxyHandlerShared(metadata, agentClassName, agentType);

  return (...args: any[]) => {
    const instance = Object.create(ctor.prototype);

    const finalPhantomId = randomUuid();
    const constructedId = shared.constructAgentId(args, finalPhantomId);

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

type ConstructedAgentId = {
  agentTypeName: string;
  constructorDataValue: DataValue;
  phantomId: Uuid | undefined;
  agentIdString: string;
};

class WasmRpxProxyHandlerShared {
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

  constructAgentId(args: any[], phantomId?: Uuid): ConstructedAgentId {
    let constructorDataValue: DataValue;

    if (args.length === 1 && this.constructorParamTypes[0].tag === 'multimodal') {
      const dataValueEither = serializeToDataValue(args[0], this.constructorParamTypes[0]);

      if (Either.isLeft(dataValueEither)) {
        throw new Error(
          `Failed to serialize multimodal constructor argument: ${dataValueEither.val}. Input is ${util.format(args)}`,
        );
      }

      constructorDataValue = dataValueEither.val;
    } else {
      const elementValues: ElementValue[] = [];
      for (const [index, arg] of args.entries()) {
        const typeInfoInternal = this.constructorParamTypes[index];

        switch (typeInfoInternal.tag) {
          case 'analysed':
            const witValue = Either.getOrThrowWith(
              WitValue.fromTsValueDefault(arg, typeInfoInternal.val),
              (err) => new Error(`Failed to encode constructor parameter ${arg}: ${err}`),
            );
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
  private readonly shared: WasmRpxProxyHandlerShared;
  private readonly agentId: AgentId;
  private readonly wasmRpc: WasmRpc;

  private readonly methodProxyCache = new Map<string, RemoteMethod<any[], any>>();

  private readonly getIdMethod: () => AgentId = () => this.agentId;
  private readonly phantomIdMethod: () => Uuid | undefined = () => {
    const [_typeName, _params, phantomId] = this.agentId.parsed();
    return phantomId;
  };
  private readonly getAgentTypeMethod: () => AgentType = () => this.shared.agentType;

  constructor(shared: WasmRpxProxyHandlerShared, constructedId: ConstructedAgentId) {
    this.shared = shared;
    this.agentId = new AgentId(constructedId.agentIdString);

    this.wasmRpc = new WasmRpc(
      constructedId.agentTypeName,
      constructedId.constructorDataValue,
      constructedId.phantomId,
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
        default:
          const methodProxy = this.methodProxyCache.get(propString);
          if (methodProxy) {
            return methodProxy;
          } else {
            const methodProxy = this.createMethodProxy(propString);
            this.methodProxyCache.set(propString, methodProxy);
            return methodProxy;
          }
      }
    }
    return undefined;
  }

  private createMethodProxy(prop: string): RemoteMethod<any[], any> {
    const methodInfo = this.shared.getMethodInfo(prop);
    const agentIdString = this.agentId.value;
    const wasmRpc = this.wasmRpc;

    async function invokeAndAwait(...fnArgs: any[]) {
      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);

      const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(methodInfo.name, inputDataValue);

      const rpcResultPollable = rpcResultFuture.subscribe();

      await rpcResultPollable.promise();

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
    }

    function invokeFireAndForget(...fnArgs: any[]) {
      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.invoke(methodInfo.name, inputDataValue);
    }

    function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
      const inputDataValue = serializeArgs(methodInfo.params, fnArgs);
      wasmRpc.scheduleInvocation(ts, methodInfo.name, inputDataValue);
    }

    const methodFn: any = (...args: any[]) => invokeAndAwait(...args);

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
        const witValue = Either.getOrThrowWith(
          WitValue.fromTsValueDefault(fnArg, param.type.val),
          (err) => new Error(`Failed to serialize arg ${param.name}: ${err}`),
        );
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
      case 'multimodal': {
        const dataValueEither = serializeToDataValue(fnArg, param.type);
        if (Either.isLeft(dataValueEither)) {
          throw new Error(
            `Failed to serialize multimodal arg ${param.name}: ${dataValueEither.val}`,
          );
        }
        // For a multimodal param, the serialized DataValue is itself the result;
        // we wrap it as a single tuple with the multimodal elements
        const multimodalDv = dataValueEither.val;
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
  return Either.getOrThrowWith(
    deserializeDataValue(
      resultDataValue,
      [
        {
          name: 'return-value',
          type: typeInfoInternal,
        },
      ],
      { tag: 'anonymous' },
    ),
    (err) => new Error(`Failed to deserialize return value of RPC call: ${err}`),
  )[0];
}
