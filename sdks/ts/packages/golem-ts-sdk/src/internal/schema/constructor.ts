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
// limitations under the License

import { ClassMetadata } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { resolveParamType } from './helpers';
import { RuntimeParam } from '../typeInfoInternal';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeScope } from '../mapping/types/scope';
import { EnrichedConstructor } from './agentType';

/**
 * Resolve the agent constructor into its schema-native enriched form, registering
 * each parameter's runtime type so the boundary can decode constructor input.
 */
export function resolveAgentConstructor(
  agentClassName: string,
  classType: ClassMetadata,
  description: string,
  promptHint: string,
): EnrichedConstructor {
  const baseError = `Schema generation failed for agent class ${agentClassName} due to unsupported types in constructor.`;

  const params: RuntimeParam[] = classType.constructorArgs.map((param) => {
    const scope = TypeScope.constructor(agentClassName, param.name, param.type.optional);
    const typeInfo = Either.getOrThrowWith(
      resolveParamType(scope, param.type),
      (err) => new Error(`${baseError} Parameter \`${param.name}\`: ${err}`),
    );

    AgentConstructorParamRegistry.setType(agentClassName, param.name, typeInfo);

    return { name: param.name, type: typeInfo };
  });

  return { name: undefined, description, promptHint, params };
}
