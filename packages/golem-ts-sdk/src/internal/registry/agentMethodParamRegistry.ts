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

type AgentClassNameString = string;
type AgentMethodNameString = string;
type AgentMethodParamNameString = string;

const agentMethodParamRegistry = new Map<
  AgentClassNameString,
  Map<
    AgentMethodNameString,
    Map<
      AgentMethodParamNameString,
      {
        analysedType?: AnalysedType;
        languageCode?: string[];
        mimeTypes?: string[];
      }
    >
  >
>();

export const AgentMethodParamRegistry = {
  ensureMeta(
    agentClassName: AgentClassName,
    method: string,
    paramName: string,
  ) {
    if (!agentMethodParamRegistry.has(agentClassName.value)) {
      agentMethodParamRegistry.set(agentClassName.value, new Map());
    }
    const classMeta = agentMethodParamRegistry.get(agentClassName.value)!;
    if (!classMeta.has(method)) {
      classMeta.set(method, new Map());
    }

    const methodMeta = classMeta.get(method)!;

    if (!methodMeta.has(paramName)) {
      methodMeta.set(paramName, {});
    }
  },

  lookup(agentClassName: AgentClassName) {
    return agentMethodParamRegistry.get(agentClassName.value);
  },

  lookupParamType(
    agentClassName: AgentClassName,
    agentMethodName: string,
  ): AnalysedType | undefined {
    const classMeta = agentMethodParamRegistry.get(agentClassName.value);
    return classMeta?.get(agentMethodName)?.get(agentMethodName)?.analysedType;
  },

  setLanguageCodes(
    agentClassName: AgentClassName,
    agentMethodName: string,
    paramName: string,
    languageCodes: string[],
  ) {
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      agentMethodName,
      paramName,
    );
    const classMeta = agentMethodParamRegistry.get(agentClassName.value)!;
    const methodMeta = classMeta.get(agentMethodName)!;
    methodMeta.get(paramName)!.languageCode = languageCodes;
  },

  setAnalysedType(
    agentClassName: AgentClassName,
    agentMethodName: string,
    paramName: string,
    analysedType: AnalysedType,
  ) {
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      agentMethodName,
      paramName,
    );
    const classMeta = agentMethodParamRegistry.get(agentClassName.value)!;
    const methodMeta = classMeta.get(agentMethodName)!;
    methodMeta.get(paramName)!.analysedType = analysedType;
  },

  setMimeTypes(
    agentClassName: AgentClassName,
    agentMethodName: string,
    paramName: string,
    mimeTypes: string[],
  ) {
    AgentMethodParamRegistry.ensureMeta(
      agentClassName,
      agentMethodName,
      paramName,
    );
    const classMeta = agentMethodParamRegistry.get(agentClassName.value)!;
    const methodMeta = classMeta.get(agentMethodName)!;
    methodMeta.get(paramName)!.mimeTypes = mimeTypes;
  },
};
