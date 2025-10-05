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
import { Datetime, WasmRpc, AgentId } from 'golem:rpc/types@0.2.2';
import * as Either from '../newTypes/either';
import * as WitValue from './mapping/values/WitValue';
import * as Option from '../newTypes/option';
import {
  getAgentType,
  makeAgentId,
  RegisteredAgentType,
} from 'golem:agent/host';
import { AgentClassName } from '../newTypes/agentClassName';
import {
  BinaryReference,
  DataValue,
  ElementValue,
  TextReference,
} from 'golem:agent/common';
import * as Value from './mapping/values/Value';
import { RemoteMethod } from '../baseAgent';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from './registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import { deserialize } from './mapping/values/deserializer';
import {
  castTsValueToBinaryReference,
  castTsValueToTextReference,
} from './mapping/values/serializer';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  ctor: T,
) {
  return (...args: any[]) => {
    const instance = new ctor(...args);

    const agentClassName = new AgentClassName(ctor.name);

    const metadataOpt = Option.fromNullable(TypeMetadata.get(ctor.name));

    if (Option.isNone(metadataOpt)) {
      throw new Error(
        `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
      );
    }

    const metadata = metadataOpt.val;

    const workerIdEither = getAgentId(agentClassName, args, metadata);

    if (Either.isLeft(workerIdEither)) {
      throw new Error(workerIdEither.val);
    }

    const workerId = workerIdEither.val;

    return new Proxy(instance, {
      get(target, prop) {
        const val = target[prop];

        if (typeof val === 'function') {
          return getMethodProxy(metadata, prop, agentClassName, workerId);
        }
        return val;
      },
    });
  };
}

function getMethodProxy(
  classMetadata: ClassMetadata,
  prop: string | symbol,
  agentClassName: AgentClassName,
  agentId: AgentId,
): RemoteMethod<any[], any> {
  const methodSignature = classMetadata.methods.get(prop.toString());

  const methodParams = methodSignature?.methodParams;

  if (!methodParams) {
    throw new Error(
      `Unresolved method ${String(
        prop,
      )} in RPC call. Make sure this method exists and is not private/protected`,
    );
  }

  const paramInfo = Array.from(methodParams);

  const methodName = prop.toString();

  const methodNameKebab = convertAgentMethodNameToKebab(methodName);

  const functionName = `${agentClassName.asWit}.{${methodNameKebab}}`;

  const returnTypeAnalysed = AgentMethodRegistry.getReturnType(
    agentClassName,
    methodName,
  );

  function serializeArgs(fnArgs: any[]): WitValue.WitValue[] {
    const parameterWitValuesEither = Either.all(
      fnArgs.map((fnArg, index) => {
        const param = paramInfo[index];
        const typeInfo = AgentMethodParamRegistry.getParamType(
          agentClassName,
          methodName,
          param[0],
        );

        if (!typeInfo) {
          throw new Error(
            `Unsupported type for parameter ${param[0]} in method ${String(
              prop,
            )}`,
          );
        }

        switch (typeInfo.tag) {
          case 'analysed':
            return WitValue.fromTsValueDefault(fnArg, typeInfo.val);

          case 'unstructured-text':
            return Either.right(WitValue.fromTsValueTextReference(fnArg));

          case 'unstructured-binary':
            return Either.right(WitValue.fromTsValueBinaryReference(fnArg));
        }
      }),
    );
    if (Either.isLeft(parameterWitValuesEither)) {
      throw new Error('Failed to encode args: ' + parameterWitValuesEither.val);
    }
    return parameterWitValuesEither.val;
  }

  async function invokeAndAwait(...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);

    const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(
      functionName,
      parameterWitValues,
    );

    const rpcResultPollable = rpcResultFuture.subscribe();
    await rpcResultPollable.promise();

    const rpcResult = rpcResultFuture.get();
    if (!rpcResult) {
      throw new Error(
        `Failed to invoke ${functionName} in agent ${agentId.agentId}`,
      );
    }

    const rpcWitValue =
      rpcResult.tag === 'err'
        ? (() => {
            throw new Error(
              'Failed to invoke: ' + JSON.stringify(rpcResult.val),
            );
          })()
        : rpcResult.val;

    if (!returnTypeAnalysed || returnTypeAnalysed.tag !== 'analysed') {
      throw new Error(
        `Return type of method ${String(prop)}  not supported in remote calls`,
      );
    }

    return deserialize(unwrapResult(rpcWitValue), returnTypeAnalysed.val);
  }

  async function invokeFireAndForget(...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);
    wasmRpc.invoke(functionName, parameterWitValues);
  }

  async function invokeSchedule(ts: Datetime, ...fnArgs: any[]) {
    const parameterWitValues = serializeArgs(fnArgs);
    const wasmRpc = new WasmRpc(agentId);
    wasmRpc.scheduleInvocation(ts, functionName, parameterWitValues);
  }

  const methodFn: any = (...args: any[]) => invokeAndAwait(...args);

  methodFn.trigger = (...args: any[]) => invokeFireAndForget(...args);
  methodFn.schedule = (ts: Datetime, ...args: any[]) =>
    invokeSchedule(ts, ...args);

  return methodFn as RemoteMethod<any[], any>;
}

// constructorArgs is an array of any, we can have more control depending on its types
// Probably this implementation is going to exist in various forms in Golem. Not sure if there
// would be a way to reuse - may be a host function that retrieves the worker-id
// given value in JSON format, and the wit-type of each value and agent-type name?
function getAgentId(
  agentClassName: AgentClassName,
  constructorArgs: any[],
  classMetadata: ClassMetadata,
): Either.Either<AgentId, string> {
  // PlaceHolder implementation that finds the container-id corresponding to the agentType!
  // We need a host function - given an agent-type, it should return a component-id as proved in the prototype.
  // But we don't have that functionality yet, hence just retrieving the current
  // component-id (for now)
  const optionalRegisteredAgentType = Option.fromNullable(
    getAgentType(agentClassName.value),
  );

  if (Option.isNone(optionalRegisteredAgentType)) {
    return Either.left(
      `There are no components implementing ${agentClassName.value}`,
    );
  }

  const registeredAgentType: RegisteredAgentType =
    optionalRegisteredAgentType.val;

  const constructorParamInfo = classMetadata.constructorArgs;

  const constructorParamTypes = constructorParamInfo.map((param) => {
    const typeInfoInternal = AgentConstructorParamRegistry.getParamType(
      agentClassName,
      param.name,
    );

    if (!typeInfoInternal) {
      throw new Error(
        `Unresolved type for constructor parameter ${param.name} in agent class ${agentClassName.value}`,
      );
    }
    return typeInfoInternal;
  });

  // It's a bit odd that the agent-id creation takes a DataValue,
  // while remote calls takes WitValue regardless of whether it is component-type
  // or unstructured-types.
  const constructorParamWitValuesResult: Either.Either<ElementValue[], string> =
    Either.all(
      constructorArgs.map((arg, index) => {
        const typeInfoInternal = constructorParamTypes[index];

        switch (typeInfoInternal.tag) {
          case 'analysed':
            return Either.map(
              WitValue.fromTsValueDefault(arg, typeInfoInternal.val),
              (witValue) => {
                let elementValue: ElementValue = {
                  tag: 'component-model',
                  val: witValue,
                };

                return elementValue;
              },
            );
          case 'unstructured-text':
            const textReference: TextReference =
              castTsValueToTextReference(arg);

            const elementValue: Either.Either<ElementValue, string> =
              Either.right({
                tag: 'unstructured-text',
                val: textReference,
              });

            return elementValue;

          case 'unstructured-binary':
            const binaryReference: BinaryReference =
              castTsValueToBinaryReference(arg);

            const elementValueBinary: Either.Either<ElementValue, string> =
              Either.right({
                tag: 'unstructured-binary',
                val: binaryReference,
              });

            return elementValueBinary;
        }
      }),
    );

  if (Either.isLeft(constructorParamWitValuesResult)) {
    throw new Error(
      'Failed to create remote agent: ' + constructorParamWitValuesResult.val,
    );
  }

  const constructorDataValue: DataValue = {
    tag: 'tuple',
    val: constructorParamWitValuesResult.val,
  };

  const agentId = makeAgentId(agentClassName.value, constructorDataValue);

  return Either.right({
    componentId: registeredAgentType.implementedBy,
    agentId: agentId,
  });
}

function convertAgentMethodNameToKebab(methodName: string): string {
  return methodName
    .replace(/([a-z])([A-Z])/g, '$1-$2')
    .replace(/[\s_]+/g, '-')
    .toLowerCase();
}

function unwrapResult(witValue: WitValue.WitValue): Value.Value {
  const value = Value.fromWitValue(witValue);

  return value.kind === 'tuple' && value.value.length > 0
    ? value.value[0]
    : value;
}
