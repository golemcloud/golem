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

import { AgentClassName } from '../../newTypes/agentClassName';
import { AnalysedType } from '../mapping/types/AnalysedType';
import * as Option from '../../newTypes/option';

type AgentClassNameString = string;
type ParamName = string;

const agentConstructorParamRegistry = new Map<
  AgentClassNameString,
  Map<
    ParamName,
    {
      languageCodes?: string[];
      mimeTypes?: string[];
      analysedType?: AnalysedType;
    }
  >
>();

export const AgentConstructorParamRegistry = {
  ensureMeta(agentClassName: AgentClassName, paramName: string) {
    if (!agentConstructorParamRegistry.has(agentClassName.value)) {
      agentConstructorParamRegistry.set(agentClassName.value, new Map());
    }
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    if (!classMeta.has(paramName)) {
      classMeta.set(paramName, {});
    }
  },

  lookup(agentClassName: AgentClassName) {
    return agentConstructorParamRegistry.get(agentClassName.value);
  },

  lookupParamType(
    agentClassName: AgentClassName,
    paramName: string,
  ): AnalysedType | undefined {
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value);
    return classMeta?.get(paramName)?.analysedType;
  },

  constructorParams(
    agentClassName: AgentClassName,
  ): [string, Option.Option<AnalysedType>][] {
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value);
    if (!classMeta) {
      return [];
    }
    return Array.from(classMeta.entries()).map(([paramName, meta]) => [
      paramName,
      Option.fromNullable(meta.analysedType),
    ]);
  },

  setLanguageCodes(
    agentClassName: AgentClassName,
    paramName: string,
    languageCodes: string[],
  ) {
    AgentConstructorParamRegistry.ensureMeta(agentClassName, paramName);
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    classMeta.get(paramName)!.languageCodes = languageCodes;
  },

  setAnalysedType(
    agentClassName: AgentClassName,
    paramName: string,
    analysedType: AnalysedType,
  ) {
    AgentConstructorParamRegistry.ensureMeta(agentClassName, paramName);
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    classMeta.get(paramName)!.analysedType = analysedType;
  },

  setMimeTypes(
    agentClassName: AgentClassName,
    paramName: string,
    mimeTypes: string[],
  ) {
    AgentConstructorParamRegistry.ensureMeta(agentClassName, paramName);
    const classMeta = agentConstructorParamRegistry.get(agentClassName.value)!;
    classMeta.get(paramName)!.mimeTypes = mimeTypes;
  },
};
