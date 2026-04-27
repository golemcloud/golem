import { Effect, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import { componentModelElement, ElementValueKindError, type ElementCodec } from "./element.js"
import type { EndpointDef } from "./http.js"
import { isMultimodal, type Multimodal, type MultimodalShape } from "./multimodal.js"
import { Principal } from "./principal.js"
import { SelfAgentId } from "./self-agent-id.js"
import { isElementSpec, type ElementSpec } from "./unstructured.js"
import { toWitCodec, type UnsupportedSchemaError, type WitCodec } from "./wit-codec.js"

/**
 * A method/constructor parameter is either an ordinary `Schema.Top` (which
 * compiles to a `component-model` element), an `ElementSpec<T>`
 * (unstructured-text/binary), or a `Multimodal<S>` (the param maps to
 * `DataSchema.multimodal`; only valid when it is the sole parameter).
 */
export type MethodParam = Schema.Top | ElementSpec<any> | Multimodal<any>

/** A record of named parameter shapes. */
export type MethodParams = Readonly<Record<string, MethodParam>>

/** Decoded user-side type for one parameter. */
export type ParamInputType<P extends MethodParam> =
  P extends Multimodal<infer S>
    ? import("./multimodal.js").MultimodalValue<S>
    : P extends ElementSpec<infer T>
      ? T
      : P extends Schema.Top
        ? P["Type"]
        : never

/** Decoded shape of a method's named-input record. */
export type MethodInput<Params extends MethodParams> = {
  readonly [K in keyof Params]: ParamInputType<Params[K]>
}

/**
 * A `MethodSpec` describes a method's wire contract — its named input
 * parameters, success type, and typed failure type — *without* an
 * implementation.
 *
 * Used inside `defineAgent({ methods })` so that the agent type can be
 * fully discovered (and its `WitCodec`s compiled) without instantiating
 * the agent. The implementation comes from the agent's `impl` block.
 */
export interface MethodSpec<
  in out Params extends MethodParams,
  in out Success extends Schema.Top,
  in out Error extends Schema.Top,
> {
  readonly params: Params
  readonly success: Success
  readonly error: Error
  /** Free-text description, surfaced as `agent-method.description`. */
  readonly description?: string
  /** Optional `prompt-hint`, surfaced as `agent-method.prompt-hint`. */
  readonly promptHint?: string
  /**
   * Optional list of HTTP endpoints exposing this method through the
   * Golem host. Compiled to `agent-method.http-endpoint`. Each endpoint
   * may bind path / query / header variables to entries of `Params`;
   * type-level constraint: every binding name must be a `keyof Params`.
   */
  readonly http?: ReadonlyArray<EndpointDef<keyof Params & string>>
}

/**
 * Build a `MethodSpec`. The `error` field defaults to `Schema.Void`
 * ("does not fail in a typed way").
 *
 * ```ts
 * const greet = method({ params: { name: Schema.String }, success: Schema.String })
 * ```
 */
export const method: {
  <const Params extends MethodParams, Success extends Schema.Top, Error extends Schema.Top>(spec: {
    readonly params: Params
    readonly success: Success
    readonly error: Error
    readonly description?: string
    readonly promptHint?: string
    readonly http?: ReadonlyArray<EndpointDef<keyof Params & string>>
  }): MethodSpec<Params, Success, Error>
  <const Params extends MethodParams, Success extends Schema.Top>(spec: {
    readonly params: Params
    readonly success: Success
    readonly description?: string
    readonly promptHint?: string
    readonly http?: ReadonlyArray<EndpointDef<keyof Params & string>>
  }): MethodSpec<Params, Success, typeof Schema.Void>
} = (spec: any): any => ({ error: Schema.Void, ...spec })

/**
 * A `Method` is a `MethodSpec` paired with a name and a body. Use
 * {@link defineMethod} to build one when you want a self-contained method
 * value (e.g. for tests, or a future "stateless functions" registry).
 *
 * Inside `defineAgent` you should use `method(...)` for the spec and
 * provide the body inside the agent's `impl` block — that gives the body
 * access to per-instance state via closure.
 */
export interface Method<
  in out Params extends MethodParams,
  in out Success extends Schema.Top,
  in out Error extends Schema.Top,
  out R,
> extends MethodSpec<Params, Success, Error> {
  readonly name: string
  readonly body: (input: MethodInput<Params>) => Effect.Effect<Success["Type"], Error["Type"], R>
}

/** Standalone Method (spec + name + body), useful outside agents. */
export const defineMethod: {
  <
    const Params extends MethodParams,
    Success extends Schema.Top,
    Error extends Schema.Top,
    R,
  >(definition: {
    readonly name: string
    readonly params: Params
    readonly success: Success
    readonly error: Error
    readonly body: (input: MethodInput<Params>) => Effect.Effect<Success["Type"], Error["Type"], R>
  }): Method<Params, Success, Error, R>
  <const Params extends MethodParams, Success extends Schema.Top, R>(definition: {
    readonly name: string
    readonly params: Params
    readonly success: Success
    readonly body: (input: MethodInput<Params>) => Effect.Effect<Success["Type"], never, R>
  }): Method<Params, Success, typeof Schema.Void, R>
} = (definition: any): any => ({ error: Schema.Void, ...definition })

/**
 * A handler implementing a `MethodSpec`: takes the decoded input record,
 * returns an Effect of the success/error types declared by the spec.
 *
 * The required-services slot allows {@link Principal} (and an optional
 * agent-specific config tag, defaulting to `never`) because the agent
 * dispatcher always provides them; users can `yield* Principal` /
 * `yield* MyConfig` inside any handler body without leaking the
 * requirement.
 */
export type Handler<S extends MethodSpec<any, any, any>, CfgTag = never> = (
  input: MethodInput<S["params"]>,
) => Effect.Effect<S["success"]["Type"], S["error"]["Type"], Principal | SelfAgentId | CfgTag>

/**
 * Invoke a standalone {@link Method} with a *decoded* input record. Useful
 * for tests; production code goes through `invokeDataValue` (with an
 * explicit handler) at the Golem boundary.
 */
export const invoke = <
  Params extends MethodParams,
  Success extends Schema.Top,
  Error extends Schema.Top,
  R,
>(
  m: Method<Params, Success, Error, R>,
  input: MethodInput<Params>,
): Effect.Effect<Success["Type"], Error["Type"], R> => m.body(input)

/**
 * Internal binding for a single method/constructor parameter slot.
 * Today every parameter is a `wire` binding (a Schema → ElementCodec)
 * or a `multimodal` binding. Runtime-injected values such as
 * {@link Principal} are not modeled as bindings at all — they reach
 * user code as Effect services provided by the dispatcher.
 */
export type ParamBinding =
  | {
      readonly kind: "wire"
      readonly name: string
      readonly element: ElementCodec<unknown>
      /**
       * The `WitCodec` backing the element when the element is a
       * component-model value. `null` for unstructured-text /
       * unstructured-binary element specs.
       */
      readonly witCodec: WitCodec<Schema.Top> | null
    }
  | {
      readonly kind: "multimodal"
      readonly name: string
      readonly multimodal: {
        readonly dataSchema: AgentCommon.DataSchema
        readonly encode: (value: any) => Effect.Effect<CoreTypes.DataValue, Schema.SchemaError>
        readonly decode: (
          dv: CoreTypes.DataValue,
        ) => Effect.Effect<any, Schema.SchemaError | import("./element.js").ElementValueKindError>
      }
      /**
       * Element shim used by legacy `wireBindings` traversals — the real
       * encode/decode lives on `multimodal.encode/decode` and consumes the
       * full `DataValue` (not a single `ElementValue`).
       */
      readonly element: ElementCodec<unknown>
      readonly witCodec: null
    }

/**
 * The compiled WIT-side encoding of a single method: per-parameter
 * bindings, the success `WitCodec` (or `null` for unit-returning methods),
 * and the matching Golem `DataSchema`s. Compiled once via
 * {@link compileMethodSpec}, then re-used per call by {@link invokeDataValue}.
 */
export interface MethodCodec<
  in out Params extends MethodParams,
  in out Success extends Schema.Top,
  in out Error extends Schema.Top,
> {
  readonly name: string
  readonly spec: MethodSpec<Params, Success, Error>
  /** Internal binding list; wire bindings expose their `WitCodec`/`ElementCodec`. */
  readonly bindings: ReadonlyArray<ParamBinding>
  /** Backwards-compatible view of `wire` bindings for existing callers. */
  readonly inputCodecs: ReadonlyArray<{
    readonly name: string
    readonly codec: WitCodec<Schema.Top>
  }>
  readonly outputCodec: WitCodec<Success> | null
  readonly outputElement: ElementCodec<Success["Type"]> | null
  readonly inputSchema: AgentCommon.DataSchema
  readonly outputSchema: AgentCommon.DataSchema
}

/** Detect a unit / `Schema.Void` return type. */
const isVoidSchema = (s: Schema.Top): boolean => s.ast._tag === "Void"

/**
 * Compile a record of named `MethodParam`s to a flat `ParamBinding[]`.
 * Reused by both `compileMethodSpec` (per-method) and `agent.ts`
 * (per-constructor) so the two share a single param-shape pipeline.
 */
export const compileParamBindings = (
  context: string,
  params: MethodParams,
): Effect.Effect<ReadonlyArray<ParamBinding>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const bindings: Array<ParamBinding> = []
    for (const [paramName, param] of Object.entries(params)) {
      if (isMultimodal(param)) {
        return yield* Effect.fail<UnsupportedSchemaError>({
          _tag: "UnsupportedSchemaError",
          reason: `${context}: multimodal parameter '${paramName}' is only allowed as a method's sole parameter`,
        } as UnsupportedSchemaError)
      }
      if (isElementSpec(param)) {
        bindings.push({
          kind: "wire",
          name: paramName,
          element: param.element as ElementCodec<unknown>,
          witCodec: null,
        })
      } else {
        const witCodec = yield* toWitCodec(param as Schema.Top)
        const element = componentModelElement(
          witCodec,
          `${context}: argument ${paramName}`,
        ) as ElementCodec<unknown>
        bindings.push({ kind: "wire", name: paramName, element, witCodec })
      }
    }
    return bindings
  })

/** Compile a method spec (name + params + success + error) to a MethodCodec. */
export const compileMethodSpec = <
  Params extends MethodParams,
  Success extends Schema.Top,
  Error extends Schema.Top,
>(
  name: string,
  spec: MethodSpec<Params, Success, Error>,
): Effect.Effect<MethodCodec<Params, Success, Error>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const paramEntries = Object.entries(spec.params)

    const outputCodec = isVoidSchema(spec.success)
      ? null
      : ((yield* toWitCodec(spec.success)) as WitCodec<Success>)
    const outputElement: ElementCodec<Success["Type"]> | null =
      outputCodec === null
        ? null
        : (componentModelElement(outputCodec, `${name}: return value`) as ElementCodec<
            Success["Type"]
          >)
    const outputSchema: AgentCommon.DataSchema =
      outputElement === null
        ? { tag: "tuple", val: [] }
        : { tag: "tuple", val: [["", outputElement.elementSchema]] }

    // Multimodal: must be the sole parameter; produces `DataSchema.multimodal`.
    const multimodalEntry = paramEntries.find(([, v]) => isMultimodal(v))
    if (multimodalEntry !== undefined) {
      if (paramEntries.length !== 1) {
        return yield* Effect.fail<UnsupportedSchemaError>({
          _tag: "UnsupportedSchemaError",
          reason: `${name}: multimodal parameters must be the sole parameter (found ${paramEntries.length})`,
        } as UnsupportedSchemaError)
      }
      const [paramName, mm] = multimodalEntry as [string, Multimodal<MultimodalShape>]
      const compiled = yield* mm.compile()
      const element: ElementCodec<unknown> = {
        // The element-schema slot for a multimodal binding is a synthetic
        // marker — actual encoding/decoding goes through the binding's
        // own `multimodal.encode/decode` paths in `invokeDataValue` and
        // the client. We store the inner shape as a component-model nil
        // here so legacy callers don't crash; the live `bindings` array
        // exposes the multimodal compiled bundle separately.
        elementSchema: {
          tag: "component-model",
          val: { nodes: [{ type: { tag: "prim-bool-type" } }] },
        } as AgentCommon.ElementSchema,
        encode: () =>
          Effect.die(new Error("multimodal binding encoded via element shim; should not happen")),
        decode: () =>
          Effect.die(new Error("multimodal binding decoded via element shim; should not happen")),
      }
      const bindings: Array<ParamBinding> = [
        {
          kind: "multimodal",
          name: paramName,
          multimodal: compiled,
          // Carry the same element shim so generic `wireBindings.map(...)`
          // codepaths still see something — they intentionally skip
          // multimodal kinds via the `kind` filter.
          element,
          witCodec: null,
        },
      ]
      const inputSchema = compiled.dataSchema
      return {
        name,
        spec,
        bindings,
        inputCodecs: [],
        outputCodec,
        outputElement,
        inputSchema,
        outputSchema,
      }
    }

    const bindings = (yield* compileParamBindings(name, spec.params)) as Array<ParamBinding>

    // Wire bindings always populate `inputSchema.tuple`.
    const wireBindings = bindings.filter(
      (b): b is Extract<ParamBinding, { kind: "wire" }> => b.kind === "wire",
    )
    // Backwards-compatible legacy view: only includes wire bindings whose
    // element is a `component-model` (i.e. backed by a `WitCodec`).
    const inputCodecs = wireBindings
      .filter((b): b is typeof b & { witCodec: WitCodec<Schema.Top> } => b.witCodec !== null)
      .map(({ name: n, witCodec }) => ({ name: n, codec: witCodec }))
    const inputSchema: AgentCommon.DataSchema = {
      tag: "tuple",
      val: wireBindings.map((b) => [b.name, b.element.elementSchema]),
    }

    return {
      name,
      spec,
      bindings,
      inputCodecs,
      outputCodec,
      outputElement,
      inputSchema,
      outputSchema,
    }
  })

/** Convenience: compile a standalone {@link Method}. */
export const compileMethod = <
  Params extends MethodParams,
  Success extends Schema.Top,
  Error extends Schema.Top,
  R,
>(
  m: Method<Params, Success, Error, R>,
): Effect.Effect<MethodCodec<Params, Success, Error>, UnsupportedSchemaError> =>
  compileMethodSpec(m.name, m)

export class InvalidDataValueError {
  readonly _tag = "InvalidDataValueError"
  constructor(readonly reason: string) {}
}

/**
 * Invoke a compiled method using a Golem `DataValue` as input and producing
 * a Golem `DataValue` as output. The actual implementation is provided
 * separately as `handler` so the same compiled codec can be paired with
 * different per-instance closures (which is how agents work).
 *
 * - Input must be the `tuple` variant whose elements line up positionally
 *   with the declared parameters; each element must be the
 *   `component-model` variant carrying a `WitValue`.
 * - Output is the `tuple` variant, with 0 elements for a unit return type
 *   and 1 element otherwise.
 */
export const invokeDataValue = <
  Params extends MethodParams,
  Success extends Schema.Top,
  Error extends Schema.Top,
  R,
>(
  mc: MethodCodec<Params, Success, Error>,
  handler: (input: MethodInput<Params>) => Effect.Effect<Success["Type"], Error["Type"], R>,
  input: CoreTypes.DataValue,
): Effect.Effect<
  CoreTypes.DataValue,
  Error["Type"] | Schema.SchemaError | InvalidDataValueError,
  R
> =>
  Effect.gen(function* () {
    // Multimodal sole-parameter case: the entire DataValue is the
    // multimodal payload, not a tuple wrapping it.
    const multimodalBinding = mc.bindings.find(
      (b): b is Extract<ParamBinding, { kind: "multimodal" }> => b.kind === "multimodal",
    )
    if (multimodalBinding !== undefined) {
      const value = yield* Effect.mapError(multimodalBinding.multimodal.decode(input), (err) =>
        err instanceof ElementValueKindError
          ? new InvalidDataValueError(
              `${mc.name}: multimodal parameter ${multimodalBinding.name} is ${err.actual}, expected ${err.expected}`,
            )
          : err,
      )
      const decoded = { [multimodalBinding.name]: value } as MethodInput<Params>
      const result = yield* handler(decoded)
      if (mc.outputElement === null) {
        return { tag: "tuple", val: [] } as CoreTypes.DataValue
      }
      const ev = yield* mc.outputElement.encode(result)
      return { tag: "tuple", val: [ev] } as CoreTypes.DataValue
    }

    if (input.tag !== "tuple") {
      return yield* Effect.fail(
        new InvalidDataValueError(`${mc.name}: expected tuple DataValue, got ${input.tag}`),
      )
    }

    const wireBindings = mc.bindings.filter(
      (b): b is Extract<ParamBinding, { kind: "wire" }> => b.kind === "wire",
    )
    if (input.val.length !== wireBindings.length) {
      return yield* Effect.fail(
        new InvalidDataValueError(
          `${mc.name}: expected ${wireBindings.length} arguments, got ${input.val.length}`,
        ),
      )
    }

    const decoded: Record<string, unknown> = {}
    for (let i = 0; i < wireBindings.length; i++) {
      const b = wireBindings[i]!
      const ev = input.val[i]!
      decoded[b.name] = yield* Effect.mapError(b.element.decode(ev), (err) =>
        err instanceof ElementValueKindError
          ? new InvalidDataValueError(
              `${mc.name}: argument ${i} (${b.name}) is ${err.actual}, expected ${err.expected}`,
            )
          : err,
      )
    }

    const result = yield* handler(decoded as MethodInput<Params>)

    if (mc.outputElement === null) {
      return { tag: "tuple", val: [] } as CoreTypes.DataValue
    }
    const ev = yield* mc.outputElement.encode(result)
    return { tag: "tuple", val: [ev] } as CoreTypes.DataValue
  }) as Effect.Effect<
    CoreTypes.DataValue,
    Error["Type"] | Schema.SchemaError | InvalidDataValueError,
    R
  >
