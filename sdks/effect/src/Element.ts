/**
 * @since 1.5.0
 */
import type { SchemaValue } from "./internal/schema-model/model.js"

/**
 * Raised when decoding an element value whose shape does not match what the
 * codec expects (e.g. an unstructured-text codec receives a non-variant value).
 *
 * In the `golem:core@2.0.0` schema model unstructured / multimodal parameters
 * project to `text` / `binary` / `variant` / `list` {@link SchemaValue} nodes
 * carrying role metadata; the per-element codecs live in
 * `./Unstructured.js` and `./Multimodal.js`.
 *
 * @since 1.5.0
 * @category errors
 */
export class ElementValueKindError {
  readonly _tag = "ElementValueKindError"
  constructor(
    readonly expected: string,
    readonly actual: SchemaValue["tag"],
    readonly context?: string,
  ) {}
}
