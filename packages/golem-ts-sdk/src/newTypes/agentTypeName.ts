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

import { AgentClassName } from './agentClassName';

export class AgentTypeName {
  readonly value: string;

  // Do NOT call this constructor casually.
  // It exists solely for converting raw wire values into an AgentTypeName which
  // can be passed down to lookup functions that takes `AgentTypeName`.
  // For all normal usage, prefer factory methods like `fromAgentClassName`.
  constructor(externalValue: string) {
    this.value = externalValue;
  }

  static fromAgentClassName(agentClassName: AgentClassName): AgentTypeName {
    return new AgentTypeName(
      convertAgentClassNameToKebab(agentClassName.value),
    );
  }
}

function convertAgentClassNameToKebab(str: string): string {
  return (
    str
      // ts classes can have _, $ and digits - therefore
      .replace(/[\d$_]+/g, '')
      .replace(/([a-z])([A-Z])/g, '$1-$2')
      .replace(/([A-Z])([A-Z][a-z])/g, '$1-$2')
      .toLowerCase()
  );
}
