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

import type { Tool, ToolError } from 'golem:tool/common@0.1.0';
import {
  deepEqual,
  type SchemaValue,
  type TypedSchemaValue,
  validateSchemaGraph,
} from '../schema-model';
import {
  type CanonicalInputField,
  encodeTool,
  type CanonicalInputValue,
  type ExtendedCommandNode,
  type ExtendedToolRuntime,
  ExtendedToolType,
  normalizeExtendedTool,
  schemaValueConforms,
} from '../tool';

export interface RegisteredTool {
  readonly extended: ExtendedToolType;
  readonly encoded: Tool;
  readonly runtime: ExtendedToolRuntime;
  readonly invoker: ToolInvoker;
}

export type ToolInvoker = (
  commandPath: readonly string[],
  input: TypedSchemaValue,
  context: unknown,
) => Promise<unknown>;

export interface PreparedToolInvocation {
  invoke(context: unknown): Promise<unknown>;
}

export interface ResolvedToolInvocation {
  readonly command: ExtendedCommandNode;
  prepare(input: TypedSchemaValue): PreparedToolInvocation;
}

interface InternalResolvedToolInvocation extends ResolvedToolInvocation {
  prepareValues(input: readonly CanonicalInputValue[]): PreparedToolInvocation;
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
      const registeredRuntime = {
        bindings: runtime.bindings.map((binding) => ({
          ...binding,
          commandPath: [...binding.commandPath],
        })),
        subtreeForwards: runtime.subtreeForwards.map((forward) => ({
          ...forward,
          pathPrefix: [...forward.pathPrefix],
        })),
      } satisfies ExtendedToolRuntime;
      const entry = {
        extended,
        encoded: encodeTool(extended),
        runtime: registeredRuntime,
        invoker: async (commandPath, input, context) => {
          const resolved = this.resolveRegistered(extended, registeredRuntime, commandPath);
          return await resolved.prepare(input).invoke(context);
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

  getInvoker(name: string): ToolInvoker | undefined {
    return this.get(name)?.invoker;
  }

  resolveInvocation(name: string, commandPath: readonly string[]): ResolvedToolInvocation {
    const registered = this.get(name);
    if (!registered) {
      throw { tag: 'invalid-tool-name', val: name } satisfies ToolError;
    }
    return this.resolveRegistered(registered.extended, registered.runtime, commandPath);
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

  private resolveRegistered(
    tool: ExtendedToolType,
    runtime: ExtendedToolRuntime,
    commandPath: readonly string[],
    invalidPath: readonly string[] = commandPath,
  ): InternalResolvedToolInvocation {
    const command = tool.commandByPath(commandPath);
    if (!command) throw invalidCommandPath(invalidPath);

    const canonicalPath = tool.commandPath(command);
    if (!canonicalPath) throw invalidCommandPath(invalidPath);

    const binding = runtime.bindings.find((candidate) =>
      pathsEqual(candidate.commandPath, canonicalPath),
    );
    if (binding) {
      const prepareValues = (
        inputValues: readonly CanonicalInputValue[],
      ): PreparedToolInvocation => {
        let projectedValues: CanonicalInputValue[];
        try {
          projectedValues = tool.canonicalInputModel(command).projectValues(inputValues);
        } catch (error) {
          throw invalidInput(error);
        }
        const handlerInput = Object.fromEntries(
          projectedValues.map((field) => [camelCase(field.name), field.value]),
        );
        return {
          invoke: async (context) =>
            await binding.handler.call(binding.receiver, handlerInput, context),
        };
      };
      return {
        command,
        prepare: (input) => prepareValues(decodeCanonicalInput(tool, command, input)),
        prepareValues,
      };
    }

    const forward = runtime.subtreeForwards.find((candidate) =>
      pathStartsWith(canonicalPath, candidate.pathPrefix),
    );
    if (!forward) throw invalidCommandPath(invalidPath);

    const child = this.get(forward.childToolName);
    if (!child) {
      throw {
        tag: 'invalid-tool-name',
        val: forward.childToolName,
      } satisfies ToolError;
    }

    const childPath = canonicalPath.slice(forward.pathPrefix.length);
    const childResolved = this.resolveRegistered(
      child.extended,
      child.runtime,
      childPath,
      invalidPath,
    );
    return {
      command,
      prepare: (input) => childResolved.prepareValues(decodeCanonicalInput(tool, command, input)),
      prepareValues: (inputValues) => childResolved.prepareValues(inputValues),
    };
  }
}

function decodeCanonicalInput(
  tool: ExtendedToolType,
  command: ExtendedCommandNode,
  input: TypedSchemaValue,
): CanonicalInputValue[] {
  const graphError = validateSchemaGraph(input.graph)[0];
  if (graphError) {
    throw invalidInput(`invalid tool input schema: ${graphError.message}`);
  }

  const inputModel = tool.canonicalInputModel(command);
  if (!deepEqual(input.graph, inputModel.codec.graph)) {
    throw invalidInput('tool input schema does not match the command canonical input schema');
  }
  const inputValue = input.value;
  if (inputValue.tag !== 'record' || inputValue.fields.length !== inputModel.fields.length) {
    throw invalidInput('tool input value does not match the command canonical input schema');
  }
  if (
    inputModel.fields.some((field, index) =>
      canonicalValueConforms(field, inputValue.fields[index]) ? false : true,
    )
  ) {
    throw invalidInput('tool input value does not match the command canonical input schema');
  }
  try {
    return inputModel.decodeValues(input.value);
  } catch (error) {
    throw invalidInput(error);
  }
}

function compareNames(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function pathsEqual(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((segment, index) => segment === right[index]);
}

function pathStartsWith(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.length <= path.length && prefix.every((segment, index) => segment === path[index]);
}

function camelCase(name: string): string {
  return name.replace(/-([a-z0-9])/g, (_, char: string) => char.toUpperCase());
}

function canonicalValueConforms(field: CanonicalInputField, value: SchemaValue): boolean {
  if (field.optionalCarrier) {
    if (value.tag !== 'option') return false;
    if (
      value.value !== undefined &&
      !schemaValueConforms(field.codec.graph, field.codec.graph.root, value.value)
    ) {
      return false;
    }
  } else if (!schemaValueConforms(field.codec.graph, field.codec.graph.root, value)) {
    return false;
  }
  try {
    return deepEqual(field.codec.toValue(field.codec.fromValue(value)), value);
  } catch {
    return false;
  }
}

function invalidCommandPath(commandPath: readonly string[]): ToolError {
  return { tag: 'invalid-command-path', val: [...commandPath] };
}

function invalidInput(error: unknown): ToolError {
  return {
    tag: 'invalid-input',
    val: error instanceof Error ? error.message : String(error),
  };
}

export const ToolRegistry: ToolRegistryImpl = new ToolRegistryImpl();
