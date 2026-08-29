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

import type {
  Doc,
  InvocationResult,
  Tool,
  ToolError,
  ToolMiddleware,
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import type { Principal } from '../../principal';
import type { UniversalToolMiddlewareInvoke } from '../../tool';
import type { ExtendedToolRuntime, ExtendedToolType } from '../tool';
import { encodeTool } from '../tool';
import {
  encodeToolInvokeError,
  invokeMonomorphicToolMiddleware,
  invokeUniversalToolMiddleware,
} from '../tool/middlewareRuntime';

interface ToolMiddlewareSourceBase {
  readonly name: string;
  readonly aliases: readonly string[];
  readonly doc: Doc;
}

export interface MonomorphicToolMiddlewareSource extends ToolMiddlewareSourceBase {
  readonly kind: 'monomorphic';
  readonly presented: ExtendedToolType;
  readonly expected: ExtendedToolType;
  readonly runtime: ExtendedToolRuntime;
}

export interface UniversalToolMiddlewareSource extends ToolMiddlewareSourceBase {
  readonly kind: 'universal';
  readonly invoke: UniversalToolMiddlewareInvoke;
}

export type ToolMiddlewareSource = MonomorphicToolMiddlewareSource | UniversalToolMiddlewareSource;

export interface RawToolMiddlewareInvocation {
  readonly toolName: string;
  readonly toolMetadata: Tool;
  readonly commandPath: readonly string[];
  readonly input: TypedSchemaValue;
  readonly stdin: AsyncIterable<number> | undefined;
  readonly principal: Principal;
  readonly wrapped: Pick<UnderlyingTool, 'invoke'>;
}

export interface RegisteredToolMiddleware {
  readonly source: ToolMiddlewareSource;
  readonly encoded: ToolMiddleware;
  invoke(invocation: RawToolMiddlewareInvocation): Promise<InvocationResult>;
}

class ToolMiddlewareRegistryImpl {
  private readonly registry = new Map<string, RegisteredToolMiddleware>();
  private readonly registrationErrors = new Map<string, string[]>();
  private readonly registrationsInProgress = new Set<string>();

  registerSource(name: string, finalize: () => ToolMiddlewareSource): RegisteredToolMiddleware {
    this.ensureNameAvailable(name);
    this.registrationsInProgress.add(name);
    try {
      const source = finalize();
      if (source.name !== name) {
        throw new Error(
          `Tool middleware registration name "${name}" does not match source name "${source.name}"`,
        );
      }
      const encoded = encodeMiddleware(source);
      const entry = {
        source,
        encoded,
        invoke: compileInvoker(source),
      } satisfies RegisteredToolMiddleware;
      this.registry.set(name, entry);
      return entry;
    } finally {
      this.registrationsInProgress.delete(name);
    }
  }

  recordRegistrationError(name: string, message: string): void {
    const messages = this.registrationErrors.get(name) ?? [];
    if (!messages.includes(message)) messages.push(message);
    this.registrationErrors.set(name, messages);
  }

  getSource(name: string): ToolMiddlewareSource | undefined {
    return this.registry.get(name)?.source;
  }

  get(name: string): RegisteredToolMiddleware | undefined {
    return this.registry.get(name);
  }

  getRegisteredMiddlewares(): ToolMiddleware[] {
    return this.sortedEntries().map(([, entry]) => entry.encoded);
  }

  getRegistrationErrors(): ReadonlyArray<{
    name: string;
    messages: readonly string[];
  }> {
    return Array.from(this.registrationErrors, ([name, messages]) => ({ name, messages })).sort(
      (left, right) => left.name.localeCompare(right.name),
    );
  }

  clearForTests(): void {
    this.registry.clear();
    this.registrationErrors.clear();
    this.registrationsInProgress.clear();
  }

  private ensureNameAvailable(name: string): void {
    if (this.registry.has(name) || this.registrationsInProgress.has(name)) {
      throw new Error(`Tool middleware "${name}" is already registered`);
    }
  }

  private sortedEntries(): [string, RegisteredToolMiddleware][] {
    return Array.from(this.registry.entries()).sort(([left], [right]) =>
      left < right ? -1 : left > right ? 1 : 0,
    );
  }
}

export const ToolMiddlewareRegistry = new ToolMiddlewareRegistryImpl();

function encodeMiddleware(source: ToolMiddlewareSource): ToolMiddleware {
  return {
    name: source.name,
    aliases: [...source.aliases],
    doc: {
      ...source.doc,
      examples: source.doc.examples.map((example) => ({ ...example })),
    },
    scope:
      source.kind === 'monomorphic'
        ? {
            tag: 'monomorphic',
            val: {
              presented: encodeTool(source.presented),
              expected: encodeTool(source.expected),
            },
          }
        : { tag: 'universal' },
  };
}

function compileInvoker(
  source: ToolMiddlewareSource,
): (invocation: RawToolMiddlewareInvocation) => Promise<InvocationResult> {
  return async (invocation) => {
    try {
      return source.kind === 'monomorphic'
        ? await invokeMonomorphicToolMiddleware(source, invocation, invocation.wrapped)
        : await invokeUniversalToolMiddleware(source, invocation, invocation.wrapped);
    } catch (error) {
      throw encodeToolInvokeError<TypedSchemaValue>(error, (payload) => payload);
    }
  };
}

export function middlewareRegistrationError(): ToolError | undefined {
  const errors = ToolMiddlewareRegistry.getRegistrationErrors();
  if (errors.length === 0) return undefined;
  return {
    tag: 'invalid-result',
    val: `Tool middleware registration failed:\n${errors
      .map(({ name, messages }) => `- Tool middleware "${name}": ${messages.join('; ')}`)
      .join('\n')}`,
  };
}
