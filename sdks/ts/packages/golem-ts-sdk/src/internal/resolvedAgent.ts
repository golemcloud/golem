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

import { Result } from 'golem:rpc/types@0.2.2';
import { AgentError, AgentType, DataValue, Principal } from 'golem:agent/common';
import { AgentId } from '../agentId';
import { AgentClassName } from '../agentClassName';
import { BaseAgent } from '../baseAgent';
import {
  AgentMethodParamMetadata,
  AgentMethodParamRegistry,
} from './registry/agentMethodParamRegistry';
import {
  deserializeDataValue,
  ParameterDetail,
  serializeToDataValue,
} from './mapping/values/dataValue';
import * as Either from '../newTypes/either';
import { AgentMethodMetadata, AgentMethodRegistry } from './registry/agentMethodRegistry';
import { createCustomError, invalidInput, invalidMethod, invalidType } from './agentError';
import { TypeInfoInternal } from './typeInfoInternal';
import { Uuid } from 'golem:agent/host';

/**
 * An AgentInternal is an internal interface that represents the basic usage of an agent
 * It is constructed only after instantiating of an agent through the AgentInitiator.
 */
export class ResolvedAgent {
  private readonly agentInstance: BaseAgent;
  private readonly agentClassName: AgentClassName;
  private readonly uniqueAgentId: AgentId;
  private readonly constructorInput: DataValue;

  private parameterMetadata: Map<string, Map<string, AgentMethodParamMetadata>> | undefined =
    undefined;
  private methodMetadata: Map<string, AgentMethodMetadata> | undefined = undefined;
  private readonly cachedMethodInfo: Map<string, CachedMethodInfo> = new Map();

  constructor(
    agentInstance: BaseAgent,
    agentClassName: AgentClassName,
    uniqueAgentId: AgentId,
    constructorInput: DataValue,
  ) {
    this.agentInstance = agentInstance;
    this.agentClassName = agentClassName;
    this.uniqueAgentId = uniqueAgentId;
    this.constructorInput = constructorInput;
  }

  getId(): AgentId {
    return this.uniqueAgentId;
  }

  phantomId(): Uuid | undefined {
    const [_typeName, _params, phantomId] = this.uniqueAgentId.parsed();
    return phantomId;
  }

  getParameters(): DataValue {
    return this.constructorInput;
  }

  getAgentType(): AgentType {
    return this.agentInstance.getAgentType();
  }

  hasCustomSnapshot(): boolean {
    const proto = Object.getPrototypeOf(this.agentInstance);
    const customSave = proto.saveSnapshot !== BaseAgent.prototype.saveSnapshot;
    const customLoad = proto.loadSnapshot !== BaseAgent.prototype.loadSnapshot;
    if (customSave !== customLoad) {
      throw new Error(
        `${this.agentInstance.constructor.name} overrides only one of saveSnapshot/loadSnapshot; ` +
          `override both or neither.`,
      );
    }
    return customSave;
  }

  loadSnapshot(bytes: Uint8Array): Promise<void> {
    return this.agentInstance.loadSnapshot(bytes);
  }

  async saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }> {
    const result = await this.agentInstance.saveSnapshot();
    if (result instanceof Uint8Array) {
      return { data: result, mimeType: 'application/octet-stream' };
    }
    return result;
  }

  async invoke(
    methodName: string,
    methodArgs: DataValue,
    principal: Principal,
  ): Promise<Result<DataValue, AgentError>> {
    const methodInfoResult = this.getCachedMethodInfo(methodName);
    if (methodInfoResult.tag === 'err') {
      return methodInfoResult;
    }
    const methodInfo = methodInfoResult.val;

    const deserializedArgs: Either.Either<any[], string> = deserializeDataValue(
      methodArgs,
      methodInfo.paramTypes,
      principal,
    );

    if (Either.isLeft(deserializedArgs)) {
      return {
        tag: 'err',
        val: invalidInput(
          `Failed to deserialize arguments for method ${methodName} in agent ${this.agentClassName.value}: ${deserializedArgs.val}`,
        ),
      };
    }

    const methodResult = await methodInfo.method.apply(this.agentInstance, deserializedArgs.val);

    // Converting the result from the method back to data-value
    const dataValueEither = serializeToDataValue(methodResult, methodInfo.returnType);

    if (Either.isLeft(dataValueEither)) {
      return {
        tag: 'err',
        val: createCustomError(
          `Failed to serialize the return value from ${methodName}: ${dataValueEither.val}`,
        ),
      };
    }

    return {
      tag: 'ok',
      val: dataValueEither.val,
    };
  }

  private getMethodParameterMetadata(
    methodName: string,
  ): Result<Map<string, AgentMethodParamMetadata>, AgentError> {
    if (!this.parameterMetadata) {
      this.parameterMetadata = AgentMethodParamRegistry.get(this.agentClassName.value);
    }

    const methodParameterMetadata = this.parameterMetadata!.get(methodName) ?? new Map();

    return {
      tag: 'ok',
      val: methodParameterMetadata,
    };
  }

  private getMethodMetadata(): Result<Map<string, AgentMethodMetadata>, AgentError> {
    if (!this.methodMetadata) {
      const methodMetadata = AgentMethodRegistry.get(this.agentClassName.value);
      if (!methodMetadata) {
        AgentMethodRegistry.debugDump();
        return {
          tag: 'err',
          val: invalidMethod(
            `Failed to retrieve method metadata for agent ${this.agentClassName.value}.`,
          ),
        };
      }
      this.methodMetadata = methodMetadata;
    }
    return {
      tag: 'ok',
      val: this.methodMetadata!,
    };
  }

  private getCachedMethodInfo(methodName: string): Result<CachedMethodInfo, AgentError> {
    const cachedInfo = this.cachedMethodInfo.get(methodName);
    if (cachedInfo) {
      return {
        tag: 'ok',
        val: cachedInfo,
      };
    } else {
      const agentMethod = (this.agentInstance as any)[methodName];

      if (!agentMethod) {
        return {
          tag: 'err',
          val: invalidMethod(
            `Method ${methodName} not found on agent ${this.agentClassName.value}`,
          ),
        };
      }

      const parameterMetadata = this.getMethodParameterMetadata(methodName);
      if (parameterMetadata.tag === 'err') {
        return parameterMetadata;
      }

      const paramTypes = [];
      for (const [paramName, paramMeta] of parameterMetadata.val) {
        if (!paramMeta.typeInfo) {
          return {
            tag: 'err',
            val: invalidType(
              `Unsupported parameter ${paramName} in method ${methodName} of agent ${this.agentClassName.value}`,
            ),
          };
        }
        paramTypes.push({
          name: paramName,
          type: paramMeta.typeInfo,
        });
      }

      const methodMetadata = this.getMethodMetadata();
      if (methodMetadata.tag === 'err') {
        return methodMetadata;
      }

      const method = methodMetadata.val.get(methodName);
      const returnType = method?.returnType;

      if (!returnType) {
        return {
          tag: 'err',
          val: invalidType(
            `Return type of method ${methodName} in agent ${this.agentClassName.value} is not supported.`,
          ),
        };
      }

      const methodInfo = {
        paramTypes,
        returnType,
        method: agentMethod,
      };
      this.cachedMethodInfo.set(methodName, methodInfo);

      return {
        tag: 'ok',
        val: methodInfo,
      };
    }
  }
}

type CachedMethodInfo = {
  paramTypes: ParameterDetail[];
  returnType: TypeInfoInternal;
  method: any;
};
