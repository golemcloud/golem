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

import { AgentError, AgentType, DataValue } from 'golem:agent/common';
import { Result } from 'golem:rpc/types@0.2.2';
import { AgentInternal } from './agentInternal';
import { AgentId } from '../agentId';
import { AgentTypeRegistry } from './registry/agentTypeRegistry';
import * as Option from 'effect/Option';
import * as AgentClassName from '../newTypes/agentClassName';

export class ResolvedAgent {
  readonly classInstance: any;
  private agentInternal: AgentInternal;
  private readonly agentClassName: AgentClassName.AgentClassName;

  constructor(
    agentClassName: AgentClassName.AgentClassName,
    tsAgentInternal: AgentInternal,
    originalInstance: any,
  ) {
    this.agentClassName = agentClassName;
    this.agentInternal = tsAgentInternal;
    this.classInstance = originalInstance;
  }

  getId(): AgentId {
    return this.agentInternal.getId();
  }

  invoke(
    methodName: string,
    args: DataValue,
  ): Promise<Result<DataValue, AgentError>> {
    return this.agentInternal.invoke(methodName, args);
  }

  getDefinition(): AgentType {
    return Option.getOrThrowWith(
      AgentTypeRegistry.lookup(this.agentClassName),
      () => new Error(`Agent class ${this.agentClassName} is not registered.`),
    );
  }
}
