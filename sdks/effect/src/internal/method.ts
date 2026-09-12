/**
 * @since 1.6.0
 */
import { Effect, Pipeable, Result, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { HostServices } from "../host/HostLive.js"
import type { EndpointDef } from "../Http.js"
import { isMultimodal, type Multimodal } from "../Multimodal.js"
import { Principal } from "../Principal.js"
import { SelfAgentId } from "../SelfAgentId.js"
import { isElementSpec, tryGetter, type ElementSpec } from "../Unstructured.js"
import {
  decodeFromWire,
  toWitCodec,
  type UnsupportedSchemaError,
  type WitCodec,
} from "../WitCodec.js"
import { witPrincipalAnnotationKey } from "../WitTypes.js"
import {
  type BindableKeys,
  type EndpointBound,
  type Invalid,
  type NoCaseFoldDuplicates,
  type NoDuplicateBindings,
  type PathBindableKeys,
  type QueryOrHeaderBindableKeys,
} from "./httpTypes.js"
import { withPipe } from "./pipeable.js"
import { composeSchemaGraphs } from "./schema-model/builder.js"
import {
  emptyMetadata,
  field,
  t,
  v,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
} from "./schema-model/model.js"
import { GraphEncoder, schemaValueToWit, schemaValueToWitAsync } from "./schema-model/wit.js"

/** @since 1.6.0 @category models */
export type MethodParam = Schema.Top | ElementSpec<any> | Multimodal<any>
/** @since 1.6.0 @category models */
export type MethodParams = Readonly<Record<string, MethodParam>>
/** @since 1.6.0 @category models */
export type MethodSuccess = Schema.Top | ElementSpec<any>
/** @since 1.6.0 @category models */
export type MethodSuccessType<S extends MethodSuccess> =
  S extends ElementSpec<infer T> ? T : S extends Schema.Top ? S["Type"] : never
/** @since 1.6.0 @category models */
export type ParamInputType<P extends MethodParam> =
  P extends Multimodal<infer S>
    ? import("../Multimodal.js").MultimodalValue<S>
    : P extends ElementSpec<infer T>
      ? T
      : P extends Schema.Top
        ? P["Type"]
        : never
/** @since 1.6.0 @category models */
export type MethodInput<Input extends MethodParams> = {
  readonly [K in keyof Input]: ParamInputType<Input[K]>
}

/** @since 1.6.0 @category models */
export type ReadOnlyOption = {
  readonly cache?: "no-cache" | "until-write" | { readonly ttlNanos: bigint }
  readonly usesPrincipal?: boolean
}

declare const methodHasHttpBrand: unique symbol

/** @since 1.6.0 @category models */
export interface MethodSpec<
  in out Input extends MethodParams,
  in out Success extends MethodSuccess,
  in out Error extends Schema.Top,
  HasHttp extends boolean = boolean,
>
  extends Pipeable.Pipeable {
  readonly [methodHasHttpBrand]?: HasHttp
  readonly input: Input
  readonly success: Success
  readonly error: Error
  readonly description?: string
  readonly promptHint?: string
  readonly http?: ReadonlyArray<EndpointDef<BindableKeys<Input>>>
  readonly readOnly?: boolean | ReadOnlyOption
}

type UnsupportedBinding<N extends string, A extends string, S extends string> = string extends N
  ? unknown
  : Exclude<N, A> extends infer X
    ? [X] extends [never]
      ? unknown
      : Invalid<`parameter '${X & string}' has a schema that is not bindable from a ${S}`>
    : unknown
type ValidateBindingSources<B extends EndpointBound, P> =
  UnsupportedBinding<B["path"][number], PathBindableKeys<P>, "path"> extends infer A
    ? A extends Invalid<string>
      ? A
      : UnsupportedBinding<
            B["query"][number],
            QueryOrHeaderBindableKeys<P>,
            "query"
          > extends infer Q
        ? Q extends Invalid<string>
          ? Q
          : UnsupportedBinding<B["header"][number], QueryOrHeaderBindableKeys<P>, "header">
        : unknown
    : unknown
type ValidateEndpointStructure<E, B, H, P> = B extends EndpointBound
  ? H extends ReadonlyArray<string>
    ? ValidateBindingSources<B, P> extends infer S
      ? S extends Invalid<string>
        ? S
        : NoDuplicateBindings<B> extends infer D
          ? D extends Invalid<string>
            ? D
            : NoCaseFoldDuplicates<H> extends infer C
              ? C extends Invalid<string>
                ? C
                : E
              : E
          : E
      : E
    : E
  : E
type ValidateEndpointsTuple<E extends ReadonlyArray<EndpointDef<string>>, P> = {
  readonly [K in keyof E]: E[K] extends EndpointDef<infer V, infer Kind, infer B, infer H>
    ? Kind extends "bodyless"
      ? [Exclude<keyof P & string, V>] extends [never]
        ? ValidateEndpointStructure<E[K], B, H, P>
        : Invalid<`GET/HEAD endpoint cannot have unbound param '${Exclude<keyof P & string, V> & string}' (only path / query / header bindings are allowed because there is no request body)`>
      : ValidateEndpointStructure<E[K], B, H, P>
    : E[K]
}
type IsNonEmptyTuple<T extends ReadonlyArray<unknown>> = T extends readonly [
  unknown,
  ...ReadonlyArray<unknown>,
]
  ? true
  : false

/** @since 1.6.0 @category constructors */
export const method: {
  <
    const Input extends MethodParams,
    Success extends Schema.Top,
    Error extends Schema.Top,
    const Eps extends ReadonlyArray<EndpointDef<BindableKeys<Input>>> = readonly [],
  >(spec: {
    readonly input: Input
    readonly success: Success
    readonly error: Error
    readonly description?: string
    readonly promptHint?: string
    readonly http?: ValidateEndpointsTuple<Eps, Input>
    readonly readOnly?: boolean | ReadOnlyOption
  }): MethodSpec<Input, Success, Error, IsNonEmptyTuple<Eps>>
  <
    const Input extends MethodParams,
    Success extends MethodSuccess,
    const Eps extends ReadonlyArray<EndpointDef<BindableKeys<Input>>> = readonly [],
  >(spec: {
    readonly input: Input
    readonly success: Success
    readonly description?: string
    readonly promptHint?: string
    readonly http?: ValidateEndpointsTuple<Eps, Input>
    readonly readOnly?: boolean | ReadOnlyOption
  }): MethodSpec<Input, Success, typeof Schema.Void, IsNonEmptyTuple<Eps>>
} = (spec: any): any => withPipe({ error: Schema.Void, ...spec })

/** @since 1.6.0 @category combinators */
export const withHttp =
  <V extends string>(...endpoints: ReadonlyArray<EndpointDef<V>>) =>
  <T extends MethodSpec<any, any, any, any>>(
    spec: T & { readonly input: Readonly<Record<V, unknown>> },
  ): T extends MethodSpec<infer I, infer S, infer E, infer _H> ? MethodSpec<I, S, E, true> : T =>
    withPipe({ ...spec, http: [...(spec.http ?? []), ...endpoints] }) as never
/** @since 1.6.0 @category combinators */
export const withDescription =
  (description: string) =>
  <T extends MethodSpec<any, any, any>>(spec: T): T =>
    withPipe({ ...spec, description }) as unknown as T
/** @since 1.6.0 @category combinators */
export const withPromptHint =
  (promptHint: string) =>
  <T extends MethodSpec<any, any, any>>(spec: T): T =>
    withPipe({ ...spec, promptHint }) as unknown as T

/** @since 1.6.0 @category models */
export interface Method<
  in out Input extends MethodParams,
  in out Success extends MethodSuccess,
  in out Error extends Schema.Top,
  out R,
> extends MethodSpec<Input, Success, Error> {
  readonly name: string
  readonly body: (
    input: MethodInput<Input>,
  ) => Effect.Effect<MethodSuccessType<Success>, Error["Type"], R>
}
/** @since 1.6.0 @category constructors */
export const defineMethod: {
  <const I extends MethodParams, S extends Schema.Top, E extends Schema.Top, R>(definition: {
    readonly name: string
    readonly input: I
    readonly success: S
    readonly error: E
    readonly body: (input: MethodInput<I>) => Effect.Effect<MethodSuccessType<S>, E["Type"], R>
  }): Method<I, S, E, R>
  <const I extends MethodParams, S extends MethodSuccess, R>(definition: {
    readonly name: string
    readonly input: I
    readonly success: S
    readonly body: (input: MethodInput<I>) => Effect.Effect<MethodSuccessType<S>, never, R>
  }): Method<I, S, typeof Schema.Void, R>
} = (definition: any): any => withPipe({ error: Schema.Void, ...definition })
/** @since 1.6.0 @category models */
export type Handler<S extends MethodSpec<any, any, any>, CfgTag = never> = (
  input: MethodInput<S["input"]>,
) => Effect.Effect<
  MethodSuccessType<S["success"]>,
  S["error"]["Type"],
  Principal | SelfAgentId | HostServices | CfgTag
>
/** @since 1.6.0 @category operations */
export const invoke = <I extends MethodParams, S extends MethodSuccess, E extends Schema.Top, R>(
  m: Method<I, S, E, R>,
  input: MethodInput<I>,
): Effect.Effect<MethodSuccessType<S>, E["Type"], R> => m.body(input)

/** A compiled named input record. @since 1.6.0 @category codecs */
export interface CompiledInputCodec<Input extends MethodParams = MethodParams> {
  readonly graph: SchemaGraph
  readonly schemaGraph: CoreTypes.SchemaGraph
  readonly inputSchema: AgentCommon.InputSchema
  readonly codec: Schema.Codec<MethodInput<Input>, SchemaValue, any, any>
  readonly encode: (
    value: MethodInput<Input>,
  ) => Effect.Effect<CoreTypes.SchemaValueTree, Schema.SchemaError, any>
  readonly encodeAsync: (
    value: MethodInput<Input>,
  ) => Effect.Effect<CoreTypes.SchemaValueTree, Schema.SchemaError, any>
  readonly decode: (
    value: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<MethodInput<Input>, Schema.SchemaError, any>
}

const isPrincipal = (schema: Schema.Top): boolean =>
  (
    SchemaAST as unknown as {
      resolveAt: <T>(key: string) => (ast: SchemaAST.AST) => T | undefined
    }
  ).resolveAt<boolean>(witPrincipalAnnotationKey)(schema.ast) === true

const paramCodec = (param: MethodParam) =>
  isMultimodal(param)
    ? param.compile()
    : isElementSpec(param)
      ? Effect.succeed(param.witCodec)
      : toWitCodec(param as Schema.Top)

/** Compile a named parameter record into one transactional record codec. @since 1.6.0 @category codecs */
export const compileParamBindings = <Input extends MethodParams>(
  context: string,
  input: Input,
): Effect.Effect<CompiledInputCodec<Input>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const entries: Array<{ name: string; codec: WitCodec<Schema.Top>; injected: boolean }> = []
    for (const [name, param] of Object.entries(input)) {
      entries.push({
        name,
        codec: (yield* paramCodec(param)) as WitCodec<Schema.Top>,
        injected: !isElementSpec(param) && !isMultimodal(param) && isPrincipal(param as Schema.Top),
      })
    }
    if (entries.some((e) => isMultimodal(input[e.name])) && entries.length !== 1) {
      return yield* Effect.fail({
        _tag: "UnsupportedSchemaError",
        reason: `${context}: multimodal parameters must be the sole parameter (found ${entries.length})`,
      } as UnsupportedSchemaError)
    }
    const composed = composeSchemaGraphs(
      entries.map((entry) => entry.codec.graph),
      context,
    )
    const graph: SchemaGraph = {
      defs: composed.defs,
      root: t.record(entries.map((entry, index) => field(entry.name, composed.roots[index]!))),
    }
    const encoder = new GraphEncoder(graph.defs)
    const roots = composed.roots.map((root) => encoder.encodeType(root))
    const schemaGraph = encoder.encodeGraphRoot(graph.root)
    const inputSchema: AgentCommon.InputSchema = {
      tag: "parameters",
      val: entries.map((e, i) => ({
        name: e.name,
        source: e.injected ? { tag: "auto-injected", val: "principal" } : { tag: "user-supplied" },
        schema: roots[i]!,
        metadata: emptyMetadata(),
      })),
    }
    const Carrier = Schema.declare((_): _ is SchemaValue => true)
    const EncodedRecord = Schema.Struct(
      Object.fromEntries(entries.map((e) => [e.name, e.codec.codec])) as Record<string, Schema.Top>,
    )
    const valueToRecord = Carrier.pipe(
      Schema.decodeTo(
        Schema.declare((_): _ is Record<string, SchemaValue> => true),
        {
          decode: tryGetter((sv: SchemaValue) => {
            if (sv.tag !== "record" || sv.fields.length !== entries.length) {
              throw new Error(`${context}: expected record with ${entries.length} fields`)
            }
            return Object.fromEntries(entries.map((e, i) => [e.name, sv.fields[i]!]))
          }),
          encode: tryGetter((record: Record<string, SchemaValue>) =>
            v.record(entries.map((e) => record[e.name]!)),
          ),
        },
      ),
    )
    const codec = valueToRecord.pipe(
      Schema.decodeTo(EncodedRecord),
    ) as unknown as CompiledInputCodec<Input>["codec"]
    const encodeValue = Schema.encodeEffect(codec)
    return {
      graph,
      schemaGraph,
      inputSchema,
      codec,
      encode: (value) => Effect.map(encodeValue(value), schemaValueToWit),
      encodeAsync: (value) =>
        Effect.flatMap(encodeValue(value), (sv) =>
          Effect.promise((signal) => schemaValueToWitAsync(sv, signal)),
        ),
      decode: (value) => decodeFromWire(codec, value),
    }
  })

/** @since 1.6.0 @category models */
export interface MethodCodec<
  I extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
> {
  readonly name: string
  readonly spec: MethodSpec<I, S, E>
  readonly graph: SchemaGraph
  readonly schemaGraph: CoreTypes.SchemaGraph
  readonly inputCodec: CompiledInputCodec<I>
  readonly outputCodec: WitCodec<Schema.Top> | undefined
  readonly inputSchema: AgentCommon.InputSchema
  readonly outputSchema: AgentCommon.OutputSchema
  readonly errorWrapped: boolean
  readonly successVoid: boolean
  readonly readOnly: AgentCommon.ReadOnlyConfig | undefined
}

const isVoidSchema = (schema: Schema.Top): boolean => schema.ast._tag === "Void"
const readOnlyConfig = (
  option: boolean | ReadOnlyOption | undefined,
): AgentCommon.ReadOnlyConfig | undefined => {
  if (!option) return undefined
  const value = option === true ? {} : option
  const cache = value.cache
  return {
    cachePolicy:
      cache === "no-cache"
        ? { tag: "no-cache" }
        : typeof cache === "object"
          ? { tag: "ttl", val: cache.ttlNanos }
          : { tag: "until-write" },
    usesPrincipal: value.usesPrincipal ?? false,
  }
}

/** Compile a complete method schema and its codecs. @since 1.6.0 @category codecs */
export const compileMethodSpec = <
  I extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
>(
  name: string,
  spec: MethodSpec<I, S, E>,
): Effect.Effect<MethodCodec<I, S, E>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const inputCodec = yield* compileParamBindings(name, spec.input)
    const errorWrapped = !isVoidSchema(spec.error)
    if (errorWrapped && isElementSpec(spec.success)) {
      return yield* Effect.fail({
        _tag: "UnsupportedSchemaError",
        reason: `${name}: an unstructured success value cannot be combined with a typed error`,
      } as UnsupportedSchemaError)
    }
    const successVoid = !isElementSpec(spec.success) && isVoidSchema(spec.success)
    const outputSchemaValue: Schema.Top | ElementSpec<any> = errorWrapped
      ? Schema.Result(successVoid ? Schema.Struct({}) : (spec.success as Schema.Top), spec.error)
      : spec.success
    const outputCodec =
      successVoid && !errorWrapped
        ? undefined
        : ((yield* paramCodec(outputSchemaValue)) as WitCodec<Schema.Top>)
    const composed = composeSchemaGraphs(
      [inputCodec.graph, ...(outputCodec ? [outputCodec.graph] : [])],
      name,
    )
    const graph: SchemaGraph = { defs: composed.defs, root: composed.roots[0]! }
    const inputRoot = composed.roots[0]!
    const outputType = outputCodec ? composed.roots[1] : undefined
    const encoder = new GraphEncoder(graph.defs)
    const inputRoots = Object.values(spec.input).map((_, i) => {
      const root = (inputRoot.body as { fields: Array<{ body: SchemaType }> }).fields[i]!.body
      return encoder.encodeType(root)
    })
    const outputRoot = outputType ? encoder.encodeType(outputType) : undefined
    const schemaGraph = encoder.finish()
    const inputSchema: AgentCommon.InputSchema = {
      ...inputCodec.inputSchema,
      val: inputCodec.inputSchema.val.map((f, i) => ({ ...f, schema: inputRoots[i]! })),
    }
    return {
      name,
      spec,
      graph,
      schemaGraph,
      inputCodec: {
        ...inputCodec,
        graph: { defs: graph.defs, root: inputRoot },
        schemaGraph,
        inputSchema,
      },
      outputCodec:
        outputCodec && outputType
          ? { ...outputCodec, graph: { defs: graph.defs, root: outputType } }
          : undefined,
      inputSchema,
      outputSchema: outputRoot === undefined ? { tag: "unit" } : { tag: "single", val: outputRoot },
      errorWrapped,
      successVoid,
      readOnly: readOnlyConfig(spec.readOnly),
    }
  })

/** @since 1.6.0 @category codecs */
export const compileMethod = <
  I extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
  R,
>(
  method: Method<I, S, E, R>,
) => compileMethodSpec(method.name, method)

/** @since 1.6.0 @category errors */
export class InvalidSchemaValueError {
  readonly _tag = "InvalidSchemaValueError"
  constructor(readonly reason: string) {}
}

/** Invoke using explicit input/output codecs and recursive schema-value trees. @since 1.6.0 @category operations */
export const invokeSchemaValue = <
  I extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
  R,
>(
  inputCodec: CompiledInputCodec<I>,
  outputCodec: WitCodec<Schema.Top> | undefined,
  options: { readonly errorWrapped: boolean; readonly successVoid: boolean },
  handler: (input: MethodInput<I>) => Effect.Effect<MethodSuccessType<S>, E["Type"], R>,
  input: CoreTypes.SchemaValueTree,
): Effect.Effect<CoreTypes.SchemaValueTree | undefined, Schema.SchemaError | E["Type"], R> =>
  Effect.gen(function* () {
    const decoded = yield* inputCodec.decode(input)
    if (options.errorWrapped) {
      const result = yield* handler(decoded).pipe(
        Effect.match({
          onFailure: (error) => Result.fail(error),
          onSuccess: (success) => Result.succeed(options.successVoid ? {} : success),
        }),
      )
      const value = yield* Schema.encodeEffect(outputCodec!.codec)(result)
      return yield* Effect.promise(() => schemaValueToWitAsync(value))
    }
    const success = yield* handler(decoded)
    if (outputCodec === undefined) return undefined
    const value = yield* Schema.encodeEffect(outputCodec.codec)(success)
    return yield* Effect.promise(() => schemaValueToWitAsync(value))
  })

/** Convenience wrapper around {@link invokeSchemaValue}. @since 1.6.0 @category operations */
export const invokeMethod = <
  I extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
  R,
>(
  codec: MethodCodec<I, S, E>,
  handler: (input: MethodInput<I>) => Effect.Effect<MethodSuccessType<S>, E["Type"], R>,
  input: CoreTypes.SchemaValueTree,
) =>
  invokeSchemaValue(
    codec.inputCodec,
    codec.outputCodec,
    { errorWrapped: codec.errorWrapped, successVoid: codec.successVoid },
    handler,
    input,
  )
