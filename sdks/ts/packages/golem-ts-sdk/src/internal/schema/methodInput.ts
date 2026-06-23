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

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import { MethodParams } from '@golemcloud/golem-ts-types-core';
import { TypeScope } from '../mapping/types/scope';
import { AgentMethodParamRegistry } from '../registry/agentMethodParamRegistry';
import { RuntimeParam } from '../typeInfoInternal';
import { resolveParamType } from './helpers';

/**
 * Resolve a method's parameters into their schema-native enriched form,
 * registering each so the boundary can decode method input.
 */
export function resolveMethodInputParams(
  agentClassName: string,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<RuntimeParam[], string> {
  const params: [string, Type.Type][] = Array.from(paramTypes);

  return Either.all(
    params.map(([parameterName, parameterType]) =>
      Either.mapError(
        Either.map(
          resolveParamType(
            TypeScope.method(methodName, parameterName, parameterType.optional),
            parameterType,
          ),
          (typeInfo) => {
            AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, typeInfo);
            return { name: parameterName, type: typeInfo };
          },
        ),
        (err) => `Method: \`${methodName}\`, Parameter: \`${parameterName}\`. Error: ${err}`,
      ),
    ),
  );
}
