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

import type { TypedSchemaValue, ToolError } from 'golem:tool/common@0.1.0';
import {
  ToolRegistry,
  resolveToolInvocation,
  type ResolvableToolRuntime,
} from '../registry/toolRegistry';
import { encodeToolValue, invalidToolResult } from './invocationResult';
import {
  preflightWitTypedSchemaValue,
  schemaGraphFromWit,
  schemaGraphsEquivalent,
  validateSchemaGraph,
  type SchemaGraph,
  type SchemaType,
} from '../schema-model';

export interface NativeProcessPreparation {
  readonly input: Record<string, unknown>;
  readonly resultTemplate: TypedSchemaValue;
  readonly stdoutRequired: boolean;
  readonly stderrRequired: boolean;
}

/** Native exports with explicit process-entry mappings call this before
 * acquiring the terminal runtime lease. This is not a generic tool invoker. It
 * validates input without invoking a handler or creating JS stream adapters.
 * The native export must select an explicit process entry, retain the supplied
 * writers, destroy the terminal runtime, and complete the declared s32 result.
 */
export async function prepareNativeProcess(
  name: string,
  commandPath: readonly string[],
  input: TypedSchemaValue,
): Promise<NativeProcessPreparation> {
  const registrationErrors = ToolRegistry.getRegistrationErrors();
  if (registrationErrors.length !== 0) {
    throw invalidToolResult('tool registration failed');
  }
  const registered = ToolRegistry.get(name);
  if (!registered) throw { tag: 'invalid-tool-name', val: name } satisfies ToolError;

  const resolved = resolveToolInvocation(
    registered.extended,
    preparationRuntime(registered),
    commandPath,
    (childName) => {
      const child = ToolRegistry.get(childName);
      return child && { extended: child.extended, runtime: preparationRuntime(child) };
    },
  );
  const body = resolved.command.body;
  if (!body) throw { tag: 'invalid-command-path', val: [...commandPath] } satisfies ToolError;
  if (body.stdin) throw invalidToolResult('native process commands cannot declare stdin');
  const resultType = body.result?.codec.graph.root.body;
  if (!body.result || resultType?.tag !== 's32' || resultType.restrictions !== undefined) {
    throw invalidToolResult('native process commands require an unrestricted s32 result');
  }
  const inputGraph = registered.extended.canonicalInputModel(resolved.command).codec.graph;
  if (!isProcessInput(inputGraph, inputGraph.root, 0)) {
    throw invalidToolResult('native process commands require plain scalar and collection inputs');
  }
  try {
    const suppliedGraph = schemaGraphFromWit(input.graph);
    if (
      validateSchemaGraph(suppliedGraph).length !== 0 ||
      !schemaGraphsEquivalent(suppliedGraph, inputGraph)
    ) {
      throw new Error('tool input schema does not match the command canonical input schema');
    }
    preflightWitTypedSchemaValue(input);
  } catch (error) {
    throw {
      tag: 'invalid-input',
      val: error instanceof Error ? error.message : String(error),
    } satisfies ToolError;
  }
  const resultTemplate = encodeToolValue(body.result.codec, 0, 'native process result');
  const prepared = resolved.prepareWire(input);
  const decoded = await prepared.invoke(undefined);
  return {
    input: decoded as Record<string, unknown>,
    resultTemplate,
    stdoutRequired: body.stdout?.required ?? false,
    stderrRequired: body.stderr?.required ?? false,
  };
}

function isProcessInput(graph: SchemaGraph, type: SchemaType, depth: number): boolean {
  if (depth > 32) return false;
  const body = type.body;
  switch (body.tag) {
    case 'string':
    case 'bool':
    case 's32':
    case 'u32':
      return true;
    case 'list':
    case 'option':
      return isProcessInput(graph, body.element, depth + 1);
    case 'record':
      return body.fields.every((field) => isProcessInput(graph, field.body, depth + 1));
    case 'ref': {
      const definition = graph.defs.get(body.id);
      return !!definition && isProcessInput(graph, definition.body, depth + 1);
    }
    default:
      return false;
  }
}

function preparationRuntime(source: ResolvableToolRuntime): ResolvableToolRuntime['runtime'] {
  return {
    bindings: source.runtime.bindings.map((binding) => ({
      commandPath: binding.commandPath,
      handler: (input) => input,
    })),
    subtreeForwards: source.runtime.subtreeForwards,
  };
}
