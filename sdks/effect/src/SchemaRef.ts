/** Immutable runtime schema views used by reflection. @since 1.6.0 */
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { SchemaGraph, SchemaType } from "./internal/schema-model/model.js"
import {
  schemaGraphFromWit,
  schemaValueFromWit,
  schemaValueToWit,
} from "./internal/schema-model/wit.js"
import {
  jsonSchema,
  packJson,
  SchemaRenderError,
  unpackJson,
  type JsonValue,
  type RenderIssue,
} from "./internal/reflection/schemaRender.js"
import {
  schemaValueConforms,
  schemaValueTreeConforms,
} from "./internal/reflection/schemaValidation.js"

/** Result of explicit schema validation. @since 1.6.0 @category models */
export type ValidationResult<T> =
  | { readonly success: true; readonly value: T }
  | { readonly success: false; readonly issues: ReadonlyArray<RenderIssue> }

/** A typed immutable facade over one root in a reflected schema graph. @since 1.6.0 @category models */
export class SchemaRef {
  readonly graph: SchemaGraph
  readonly root: SchemaType
  constructor(wireGraph: CoreTypes.SchemaGraph, root = wireGraph.root) {
    const decoded = schemaGraphFromWit({ ...wireGraph, root })
    this.graph = deepFreeze(decoded)
    this.root = this.graph.root
    Object.freeze(this)
  }
  /** Pack canonical JSON into the native schema-value carrier. @since 1.6.0 @category conversions */
  packJson(value: JsonValue): CoreTypes.SchemaValueTree {
    return schemaValueToWit(packJson(this.graph, this.root, value))
  }
  /** Unpack a native schema value into canonical JSON. @since 1.6.0 @category conversions */
  unpackJson(value: CoreTypes.SchemaValueTree): JsonValue {
    return unpackJson(this.graph, this.root, schemaValueFromWit(value))
  }
  /** Explicitly validate canonical JSON. @since 1.6.0 @category validation */
  validateJson(value: JsonValue): ValidationResult<CoreTypes.SchemaValueTree> {
    try {
      const model = packJson(this.graph, this.root, value)
      if (!schemaValueConforms(this.graph, this.root, model))
        return invalid("schema value does not conform to the expected schema")
      return { success: true, value: schemaValueToWit(model) }
    } catch (error) {
      return invalid(error)
    }
  }
  /**
   * Validate WIT membership and restrictions, independently of JSON representability.
   * Unrestricted native floats include NaN and infinities; native integers may exceed
   * the safe JSON range. Use validateJson or unpackJson for canonical JSON boundaries.
   * @since 1.6.0 @category validation
   */
  validateValue(value: CoreTypes.SchemaValueTree): ValidationResult<CoreTypes.SchemaValueTree> {
    return schemaValueTreeConforms(this.graph, this.root, value)
      ? { success: true, value }
      : invalid("schema value does not conform to the expected schema")
  }
  /** Render this root as JSON Schema. @since 1.6.0 @category conversions */
  toJsonSchema(options: { readonly includeDraftMarker?: boolean } = {}): JsonValue {
    const rendered = jsonSchema(this.graph, this.root, false) as Record<string, JsonValue>
    return options.includeDraftMarker === false
      ? rendered
      : { $schema: "https://json-schema.org/draft/2020-12/schema", ...rendered }
  }
}

const invalid = <T>(error: unknown): ValidationResult<T> => ({
  success: false,
  issues: [
    {
      path: error instanceof SchemaRenderError ? error.path : [],
      message: error instanceof Error ? error.message : String(error),
    },
  ],
})
const deepFreeze = <T>(value: T, seen = new WeakSet<object>()): T => {
  if (value === null || typeof value !== "object" || seen.has(value)) return value
  seen.add(value)
  if (value instanceof Map) {
    for (const [key, item] of value) {
      deepFreeze(key, seen)
      deepFreeze(item, seen)
    }
    Object.defineProperties(value, {
      set: {
        value: () => {
          throw new TypeError("SchemaRef maps are immutable")
        },
      },
      delete: {
        value: () => {
          throw new TypeError("SchemaRef maps are immutable")
        },
      },
      clear: {
        value: () => {
          throw new TypeError("SchemaRef maps are immutable")
        },
      },
    })
  } else if (!(value instanceof Uint8Array))
    for (const item of Object.values(value)) deepFreeze(item, seen)
  return Object.freeze(value)
}

export { SchemaRenderError, type JsonValue, type RenderIssue }
