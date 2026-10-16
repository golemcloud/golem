import type { SchemaValue } from "../schema-model/model.js"

const overlays = new WeakMap<object, ReadonlyMap<string, SchemaValue>>()

/**
 * Attach root record field defaults to a schema view. The defaults apply only to its canonical
 * JSON surface: omitted fields are filled when packing JSON, and the rendered JSON Schema drops
 * them from `required` and declares their canonical JSON default. Native values are unaffected.
 */
export const attachJsonFieldDefaults = (
  schema: object,
  defaults: ReadonlyMap<string, SchemaValue>,
): void => {
  if (defaults.size > 0) overlays.set(schema, defaults)
}

export const jsonFieldDefaults = (schema: object): ReadonlyMap<string, SchemaValue> | undefined =>
  overlays.get(schema)
