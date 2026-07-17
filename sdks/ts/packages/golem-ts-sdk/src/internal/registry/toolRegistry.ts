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

import type { Tool } from 'golem:tool/common@0.1.0';
import {
  encodeTool,
  type ExtendedToolRuntime,
  ExtendedToolType,
  normalizeExtendedTool,
} from '../tool';

export interface RegisteredTool {
  readonly extended: ExtendedToolType;
  readonly encoded: Tool;
  readonly runtime: ExtendedToolRuntime;
}

class ToolRegistryImpl {
  private readonly registry = new Map<string, RegisteredTool>();
  private readonly registrationErrors = new Map<string, string[]>();
  private readonly registrationsInProgress = new Set<string>();

  register(tool: ExtendedToolType, runtime: ExtendedToolRuntime): RegisteredTool {
    return this.registerImplementation(tool.toolName, () => ({ tool, runtime }));
  }

  registerImplementation(
    name: string,
    finalize: () => { readonly tool: ExtendedToolType; readonly runtime: ExtendedToolRuntime },
  ): RegisteredTool {
    this.ensureNameAvailable(name);
    this.registrationsInProgress.add(name);
    try {
      const { tool, runtime } = finalize();
      if (tool.toolName !== name) {
        throw new Error(
          `Tool registration name "${name}" does not match descriptor name "${tool.toolName}"`,
        );
      }
      const extended = normalizeExtendedTool(tool);
      const entry = {
        extended,
        encoded: encodeTool(extended),
        runtime: {
          bindings: runtime.bindings.map((binding) => ({
            ...binding,
            commandPath: [...binding.commandPath],
          })),
          subtreeForwards: runtime.subtreeForwards.map((forward) => ({
            ...forward,
            pathPrefix: [...forward.pathPrefix],
          })),
        },
      } satisfies RegisteredTool;

      this.registry.set(name, entry);
      return entry;
    } finally {
      this.registrationsInProgress.delete(name);
    }
  }

  getRegisteredTools(): Tool[] {
    return this.sortedEntries().map(([, entry]) => entry.encoded);
  }

  get(name: string): RegisteredTool | undefined {
    return this.registry.get(name);
  }

  getTool(name: string): Tool | undefined {
    return this.get(name)?.encoded;
  }

  getExtendedTool(name: string): ExtendedToolType | undefined {
    return this.get(name)?.extended;
  }

  getRuntime(name: string): ExtendedToolRuntime | undefined {
    return this.get(name)?.runtime;
  }

  recordRegistrationError(toolName: string, message: string): void {
    const messages = this.registrationErrors.get(toolName) ?? [];
    if (!messages.includes(message)) messages.push(message);
    this.registrationErrors.set(toolName, messages);
  }

  getRegistrationError(toolName: string): readonly string[] | undefined {
    return this.registrationErrors.get(toolName);
  }

  getRegistrationErrors(): ReadonlyArray<{
    toolName: string;
    messages: readonly string[];
  }> {
    return Array.from(this.registrationErrors, ([toolName, messages]) => ({
      toolName,
      messages,
    })).sort((left, right) => compareNames(left.toolName, right.toolName));
  }

  clearForTests(): void {
    this.registry.clear();
    this.registrationErrors.clear();
    this.registrationsInProgress.clear();
  }

  private ensureNameAvailable(name: string): void {
    if (this.registry.has(name) || this.registrationsInProgress.has(name)) {
      throw new Error(`Tool "${name}" is already registered`);
    }
  }

  private sortedEntries(): [string, RegisteredTool][] {
    return Array.from(this.registry.entries()).sort(([left], [right]) => compareNames(left, right));
  }
}

function compareNames(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

export const ToolRegistry: ToolRegistryImpl = new ToolRegistryImpl();
