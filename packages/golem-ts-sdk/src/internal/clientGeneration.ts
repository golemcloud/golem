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

import {
  ClassMetadata,
  Type,
  TypeMetadata,
} from '@golemcloud/golem-ts-types-core';
import { WasmRpc, WorkerId } from 'golem:rpc/types@0.2.2';
import * as Either from '../newTypes/either';
import * as WitValue from './mapping/values/WitValue';
import * as Option from '../newTypes/option';
import {
  getAgentType,
  makeAgentId,
  RegisteredAgentType,
} from 'golem:agent/host';
import { AgentTypeName } from '../newTypes/agentTypeName';
import { AgentClassName } from '../newTypes/agentClassName';
import { DataValue, ElementValue } from 'golem:agent/common';
import * as Value from './mapping/values/Value';

export function getRemoteClient<T extends new (...args: any[]) => any>(
  ctor: T,
) {
  return (...args: any[]) => {
    const instance = new ctor(...args);

    const agentClassName = new AgentClassName(ctor.name);
    const agentTypeName = AgentTypeName.fromAgentClassName(agentClassName);

    const metadataOpt = Option.fromNullable(TypeMetadata.get(ctor.name));

    if (Option.isNone(metadataOpt)) {
      throw new Error(
        `Metadata for agent class ${ctor.name} not found. Make sure this agent class extends BaseAgent and is registered using @agent decorator`,
      );
    }

    const metadata = metadataOpt.val;

    const workerIdEither = getWorkerId(agentTypeName, args, metadata);

    if (Either.isLeft(workerIdEither)) {
      throw new Error(workerIdEither.val);
    }

    const workerId = workerIdEither.val;

    return new Proxy(instance, {
      get(target, prop) {
        const val = target[prop];

        if (typeof val === 'function') {
          return getMethodProxy(metadata, prop, agentTypeName, workerId);
        }
        return val;
      },
    });
  };
}

function getMethodProxy(
  classMetadata: ClassMetadata,
  prop: string | symbol,
  agentTypeName: AgentTypeName,
  workerId: WorkerId,
) {
  const methodSignature = classMetadata.methods.get(prop.toString());

  const methodParams = methodSignature?.methodParams;

  if (!methodParams) {
    throw new Error(
      `Method ${String(
        prop,
      )} not found in metadata. Make sure this method exists and is not private/protected`,
    );
  }

  const paramInfo = Array.from(methodParams);

  const returnType = methodSignature?.returnType;

  return async (...fnArgs: any[]) => {
    const methodNameKebab = convertAgentMethodNameToKebab(prop.toString());
    const functionName = `${agentTypeName.value}.{${methodNameKebab}}`;

    const parameterWitValuesEither = Either.all(
      fnArgs.map((fnArg, index) => {
        const param = paramInfo[index];
        const typ = param[1];
        return WitValue.fromTsValue(fnArg, typ);
      }),
    );

    const parameterWitValues = Either.isLeft(parameterWitValuesEither)
      ? (() => {
          throw new Error(
            'Failed to create remote agent: ' +
              JSON.stringify(parameterWitValuesEither.val),
          );
        })()
      : parameterWitValuesEither.val;

    const wasmRpc = new WasmRpc(workerId);

    const rpcResultFuture = wasmRpc.asyncInvokeAndAwait(
      functionName,
      parameterWitValues,
    );

    const rpcResultPollable = rpcResultFuture.subscribe();

    await rpcResultPollable.promise();

    const rpcResult = rpcResultFuture.get();

    if (!rpcResult) {
      throw new Error(
        `Failed to invoke ${functionName} in agent ${workerId.workerName}. RPC result is not available after polling`,
      );
    }

    const rpcWitValue =
      rpcResult.tag === 'err'
        ? (() => {
            throw new Error(
              'Failed to invoke function: ' + JSON.stringify(rpcResult.val),
            );
          })()
        : rpcResult.val;

    return Value.toTsValue(unwrapResult(rpcWitValue), returnType);
  };
}

// constructorArgs is an array of any, we can have more control depending on its types
// Probably this implementation is going to exist in various forms in Golem. Not sure if there
// would be a way to reuse - may be a host function that retrieves the worker-id
// given value in JSON format, and the wit-type of each value and agent-type name?
function getWorkerId(
  agentTypeName: AgentTypeName,
  constructorArgs: any[],
  classMetadata: ClassMetadata,
): Either.Either<WorkerId, string> {
  // PlaceHolder implementation that finds the container-id corresponding to the agentType!
  // We need a host function - given an agent-type, it should return a component-id as proved in the prototype.
  // But we don't have that functionality yet, hence just retrieving the current
  // component-id (for now)
  const optionalRegisteredAgentType = Option.fromNullable(
    getAgentType(agentTypeName.value),
  );

  if (Option.isNone(optionalRegisteredAgentType)) {
    return Either.left(`There are no components implementing ${agentTypeName}`);
  }

  const registeredAgentType: RegisteredAgentType =
    optionalRegisteredAgentType.val;

  const constructorParamInfo = classMetadata.constructorArgs;

  const constructorParamTypes = constructorParamInfo.map((param) => param.type);

  const constructorParamWitValuesResult: Either.Either<ElementValue[], string> =
    Either.all(
      constructorArgs.map((arg, index) => {
        const typ = constructorParamTypes[index];
        return Either.map(WitValue.fromTsValue(arg, typ), (witValue) => {
          let elementValue: ElementValue = {
            tag: 'component-model',
            val: witValue,
          };

          return elementValue;
        });
      }),
    );

  if (Either.isLeft(constructorParamWitValuesResult)) {
    throw new Error(
      'Failed to create remote agent: ' +
        JSON.stringify(constructorParamWitValuesResult.val),
    );
  }

  const constructorDataValue: DataValue = {
    tag: 'tuple',
    val: constructorParamWitValuesResult.val,
  };

  const agentId = makeAgentId(agentTypeName.value, constructorDataValue);

  if (agentId.tag === 'err') {
    return Either.left(
      `Failed to create agent-id for agent type ${agentTypeName.value}: ${JSON.stringify(agentId.val)}`,
    );
  }

  return Either.right({
    componentId: registeredAgentType.implementedBy,
    workerName: agentId.val,
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

  const innerResult =
    value.kind === 'tuple' && value.value.length > 0 ? value.value[0] : value;

  return innerResult.kind === 'result'
    ? innerResult.value.ok
      ? innerResult.value.ok
      : (() => {
          throw new Error(
            `Remote invocation failed: ${JSON.stringify(
              innerResult.value.err,
            )}`,
          );
        })()
    : innerResult;
}
