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

import { convertTypeNameToKebab } from './internal/mapping/types/stringFormat';

export class AgentClassName {
  readonly value: string;
  readonly asWit: string;

  constructor(agentClassName: string) {
    validateAgentClassName(agentClassName);
    this.value = agentClassName;
    this.asWit = convertTypeNameToKebab(agentClassName);
  }
}

function validateAgentClassName(agentClassName: string): void {
  if (agentClassName.length === 0) {
    throw new Error('Agent class name cannot be empty');
  }

  if (!/^[a-zA-Z0-9_-]+$/.test(agentClassName)) {
    throw new Error(
      `Agent class name '${agentClassName}' must contain only ASCII letters, numbers, underscores, and dashes`,
    );
  }

  if (/__/.test(agentClassName) || /--/.test(agentClassName)) {
    throw new Error(
      `Agent class name '${agentClassName}' cannot contain consecutive underscores or dashes`,
    );
  }

  if (
    agentClassName.startsWith('_') ||
    agentClassName.endsWith('_') ||
    agentClassName.startsWith('-') ||
    agentClassName.endsWith('-')
  ) {
    throw new Error(
      `Agent class name '${agentClassName}' cannot start or end with underscore or dash`,
    );
  }

  const parts = agentClassName.split(/[_-]/);
  for (const part of parts) {
    if (/^\d/.test(part)) {
      throw new Error(`Agent class name '${agentClassName}' segments cannot start with a number`);
    }
  }

  if (!/^[a-zA-Z]/.test(agentClassName)) {
    throw new Error(`Agent class name '${agentClassName}' must start with a letter`);
  }
}
