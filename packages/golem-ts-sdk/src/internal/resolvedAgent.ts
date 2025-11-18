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
import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { AgentId } from '../agentId';
import { AgentClassName } from '../newTypes/agentClassName';
import { BaseAgent } from '../baseAgent';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentMethodParamRegistry } from './registry/agentMethodParamRegistry';
import {
  deserializeDataValue,
  serializeToDataValue,
} from './mapping/values/dataValue';
import * as Either from '../newTypes/either';
import { AgentMethodRegistry } from './registry/agentMethodRegistry';
import { createCustomError } from './agentError';

/**
 * An AgentInternal is an internal interface that represents the basic usage of an agent
 * It is constructed only after instantiating of an agent through the AgentInitiator.
 */
export class ResolvedAgent {
  private readonly agentInstance: BaseAgent;
  private readonly agentClassName: AgentClassName;
  private readonly uniqueAgentId: AgentId;
  private readonly constructorInput: DataValue;

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

  getParameters(): DataValue {
    return this.constructorInput;
  }

  getAgentType(): AgentType {
    return this.agentInstance.getAgentType();
  }

  loadSnapshot(bytes: Uint8Array): Promise<void> {
    return this.agentInstance.loadSnapshot(bytes);
  }

  async saveSnapshot(): Promise<Uint8Array> {
    return await this.agentInstance.saveSnapshot();
  }

  async invoke(
    methodName: string,
    methodArgs: DataValue,
  ): Promise<Result<DataValue, AgentError>> {
    const agentMethod = (this.agentInstance as any)[methodName];

    if (!agentMethod)
      throw new Error(
        `Method ${methodName} not found on agent ${this.agentClassName.value}`,
      );

    const methodParams = TypeMetadata.get(
      this.agentClassName.value,
    )?.methods.get(methodName)?.methodParams;

    if (!methodParams) {
      const error: AgentError = {
        tag: 'invalid-method',
        val: `Failed to retrieve parameter types for method ${methodName} in agent ${this.agentClassName.value}.`,
      };
      return {
        tag: 'err',
        val: error,
      };
    }

    const methodParamTypes = Array.from(methodParams.entries()).map((param) => {
      const paramName = param[0];

      const paramTypeInfo = AgentMethodParamRegistry.getParamType(
        this.agentClassName.value,
        methodName,
        paramName,
      );

      if (!paramTypeInfo) {
        throw new Error(
          `Internal error: Unsupported parameter ${paramName} in method ${methodName} of agent ${this.agentClassName.value}`,
        );
      }

      return {
        parameterName: paramName,
        parameterTypeInfo: paramTypeInfo,
      };
    });

    const deserializedArgs: Either.Either<any[], string> = deserializeDataValue(
      methodArgs,
      methodParamTypes,
    );

    if (Either.isLeft(deserializedArgs)) {
      const error: AgentError = {
        tag: 'invalid-input',
        val: `Failed to deserialize arguments for method ${methodName} in agent ${this.agentClassName.value}: ${deserializedArgs.val}`,
      };

      return {
        tag: 'err',
        val: error,
      };
    }

    const methodResult = await agentMethod.apply(
      this.agentInstance,
      deserializedArgs.val,
    );

    const agentType = this.agentInstance.getAgentType();

    const methodSignature = agentType.methods.find(
      (m) => m.name === methodName,
    );

    if (!methodSignature) {
      const error: AgentError = {
        tag: 'invalid-method',
        val: `Method ${methodName} not found in agent type ${this.agentClassName.value}`,
      };

      return {
        tag: 'err',
        val: error,
      };
    }

    const returnTypeAnalysed = AgentMethodRegistry.getReturnType(
      this.agentClassName.value,
      methodName,
    );

    if (!returnTypeAnalysed) {
      const error: AgentError = {
        tag: 'invalid-type',
        val: `Return type of method ${methodName} in agent ${this.agentClassName.value} is not supported.`,
      };

      return {
        tag: 'err',
        val: error,
      };
    }

    // Converting the result from method back to data-value
    const dataValueEither = serializeToDataValue(
      methodResult,
      returnTypeAnalysed,
    );

    if (Either.isLeft(dataValueEither)) {
      const agentError = createCustomError(
        `Failed to serialize the return value from ${methodName}: ${dataValueEither.val}`,
      );

      return {
        tag: 'err',
        val: agentError,
      };
    }

    return {
      tag: 'ok',
      val: dataValueEither.val,
    };
  }
}
