/**
 * @since 0.1.0
 */
import { Effect, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type { WitCodec } from "./WitCodec.js"

/**
 * `ElementCodec<T>` is the boundary between an Effect Schema-driven user
 * value of type `T` and a Golem `ElementValue` (one slot of a `DataValue`
 * tuple or multimodal payload).
 *
 * It abstracts over the **kind** of element schema:
 *
 * - `component-model` — the value is encoded via a `WitCodec` and carried
 *   as a `WitValue`. This is what every "ordinary" `Schema.Top` collapses
 *   to (see {@link componentModelElement}).
 * - `unstructured-text` / `unstructured-binary` — the value is a
 *   `TextReference` / `BinaryReference` carried directly as the
 *   element-value payload; no `WitValue` is involved. Implemented by
 *   factories under `effect-golem` (see step 6/7 of the gap plan).
 *
 * Keeping the codec narrow at this layer means the rest of the SDK
 * (method/agent/client) speaks a single uniform shape per parameter slot
 * regardless of which element kind it is.
 *
 * @since 0.1.0
 * @category codecs
 */
export interface ElementCodec<T> {
  /** The schema as it appears inside a `DataSchema.tuple` / `DataSchema.multimodal`. */
  readonly elementSchema: AgentCommon.ElementSchema
  /** Encode a user-side value to an `ElementValue` for the host call. */
  readonly encode: (value: T) => Effect.Effect<CoreTypes.ElementValue, Schema.SchemaError>
  /** Decode an incoming `ElementValue` back into a user-side value. */
  readonly decode: (
    element: CoreTypes.ElementValue,
  ) => Effect.Effect<T, Schema.SchemaError | ElementValueKindError>
}

/**
 * Raised by `ElementCodec.decode` when the incoming `ElementValue.tag`
 * doesn't match what the codec expects (e.g. a `component-model` codec
 * receives an `unstructured-text` value).
 *
 * @since 0.1.0
 * @category errors
 */
export class ElementValueKindError {
  readonly _tag = "ElementValueKindError"
  constructor(
    readonly expected: AgentCommon.ElementSchema["tag"],
    readonly actual: CoreTypes.ElementValue["tag"],
    readonly context?: string,
  ) {}
}

/**
 * Lift a `WitCodec<S>` to the `ElementCodec<S["Type"]>` that wraps the
 * underlying `WitValue` in `{ tag: "component-model", val: ... }`.
 *
 * @since 0.1.0
 * @category constructors
 */
export const componentModelElement = <S extends Schema.Top>(
  witCodec: WitCodec<S>,
  context?: string,
): ElementCodec<S["Type"]> => ({
  elementSchema: witCodec.elementSchema,
  encode: (value) =>
    Effect.map(
      Schema.encodeEffect(witCodec.codec as Schema.Codec<S["Type"], any, never, never>)(value),
      (wv) => ({ tag: "component-model", val: wv }) as CoreTypes.ElementValue,
    ),
  decode: (element) => {
    if (element.tag !== "component-model") {
      return Effect.fail(new ElementValueKindError("component-model", element.tag, context))
    }
    return Schema.decodeEffect(witCodec.codec as Schema.Codec<S["Type"], any, never, never>)(
      element.val,
    )
  },
})
