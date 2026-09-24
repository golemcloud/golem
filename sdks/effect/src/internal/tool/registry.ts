import type { Layer } from "effect"
import {
  compileDefinition,
  implementationAt,
  type ImplementedTool,
  type Registered,
  type ToolDefinition,
  type ToolImplementation,
} from "./model.js"

const registry = new Map<string, Registered>()

export function registerTool<N extends string>(
  definition: ToolDefinition<N>,
  implementation: ToolImplementation,
  layer?: Layer.Layer<any>,
): ImplementedTool<N> {
  if (registry.has(definition.name))
    throw new Error(`Tool '${definition.name}' is already registered`)
  return registerCompiledTool(
    compileDefinition(definition),
    implementation,
    layer,
  ) as ImplementedTool<N>
}

export function registerCompiledTool(
  compiled: Omit<Registered, "implementation" | "layer">,
  implementation: ToolImplementation,
  layer?: Layer.Layer<any>,
): ImplementedTool<string> {
  const definition = compiled.definition
  if (registry.has(definition.name))
    throw new Error(`Tool '${definition.name}' is already registered`)
  for (const path of compiled.bodies.keys()) {
    if (!implementationAt(implementation, definition.name, path ? path.split("/") : []))
      throw new Error(`missing implementation for tool command '${path || definition.name}'`)
  }
  registry.set(definition.name, { ...compiled, implementation, layer })
  return { name: definition.name, definition }
}
export const registeredTools = () => [...registry.values()]
export const resetTools = () => registry.clear()
