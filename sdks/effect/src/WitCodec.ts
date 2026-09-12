/**
 * @since 1.6.0
 *
 * Compiles an Effect Schema into the `golem:core/types@2.0.0` schema model:
 * a recursive {@link SchemaType} (the WIT type) plus a bidirectional value
 * codec to/from {@link SchemaValue}. The flat wire carriers
 * (`schema-graph` / `schema-value-tree`) are produced from these by
 * `./internal/schema-model/wit.ts` at the dispatch boundary.
 */
import {
  Effect,
  HashMap,
  Option,
  Result,
  Schema,
  SchemaAST,
  SchemaGetter,
  SchemaIssue,
} from "effect"
import {
  field,
  emptyMetadata,
  schemaType,
  t,
  v,
  variantCase,
  type NumericBound,
  type NumericRestrictions,
  type SchemaGraph,
  type MetadataEnvelope,
  type SchemaType,
  type SchemaValue,
  type UnionBranch,
  type VariantCaseType,
} from "./internal/schema-model/model.js"
import { validateSchemaGraph } from "./internal/schema-model/validation.js"
import {
  GuestQuotaTokenHandle,
  peekGuestQuotaTokenHandle,
} from "./internal/schema-model/quotaTokenHandle.js"
import { QUOTA_INTERNAL } from "./internal/schema-model/quotaInternal.js"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import {
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
  schemaValueToWitAsync,
} from "./internal/schema-model/wit.js"
import { withCapabilityTransaction } from "./internal/schema-model/capabilityTransaction.js"
import {
  variantCaseNameAnnotationKey,
  witNumericRestrictionsKey,
  witPrincipalAnnotationKey,
  witQuotaTokenAnnotationKey,
  witTypeAnnotationKey,
  witTypedArrayAnnotationKey,
  witMetadataAnnotationKey,
  witSchemaNodeAnnotationKey,
  type NumericRestrictionsInput,
  type WitSchemaNode,
  type WitNumericKind,
  type WitTypedArrayKind,
} from "./WitTypes.js"
import { AgentStream, agentStreamFromHandle, agentStreamToHandle } from "./internal/agentStream.js"
import { GuestSecretHandle, peekGuestSecretHandle } from "./internal/schema-model/secretHandle.js"
import { SECRET_INTERNAL } from "./internal/schema-model/secretInternal.js"
import {
  GuestPermissionCardHandle,
  peekGuestPermissionCardHandle,
} from "./internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "./internal/schema-model/permissionCardInternal.js"

// Branded so `Durability.wrap` (and any other downstream consumer that uses
// nominal SDK-error detection) can route this into the defect channel without
// `_tag`-string sniffing. `Symbol.for(...)` guarantees the same runtime symbol
// across modules.
const sdkErrorBrand: unique symbol = Symbol.for("effect-golem/durable-function/sdk-error")

/**
 * Raised by {@link toWitCodec} (and registration helpers that compile a user
 * schema) when an Effect Schema construct cannot be represented in the Golem
 * schema model.
 *
 * @since 1.6.0
 * @category errors
 */
export class UnsupportedSchemaError {
  readonly _tag = "UnsupportedSchemaError"
  readonly [sdkErrorBrand] = true
  constructor(readonly reason: string) {}
}

/**
 * A pair of pure transforms mirroring a single AST node onto its
 * {@link SchemaValue} representation. They operate on the user schema's
 * **encoded** values, never the decoded domain values — so the user's own
 * decode/encode logic (refinements, transformations, branded types, …) still
 * runs when the full codec is evaluated.
 */
interface ValuePair {
  /** Encoded → schema value. Called during `Schema.encode`. */
  readonly toValue: (encoded: any) => SchemaValue
  /** Schema value → encoded. Called during `Schema.decode`. */
  readonly fromValue: (value: SchemaValue) => any
}

let conversionContext: ReturnType<typeof Effect.context<any>> extends Effect.Effect<
  infer C,
  any,
  any
>
  ? C | undefined
  : never

const withConversionContext = <A>(
  context: NonNullable<typeof conversionContext>,
  f: () => A,
): A => {
  const previous = conversionContext
  conversionContext = context
  try {
    return f()
  } finally {
    conversionContext = previous
  }
}

const requireConversionContext = (): NonNullable<typeof conversionContext> => {
  if (conversionContext === undefined)
    throw new Error("stream conversion requires an Effect context")
  return conversionContext
}

/**
 * The full mapping for a single Effect Schema:
 *
 * - `graph`  — a self-contained `SchemaGraph` (`root` schema type + nominal
 *              defs; defs are empty for the structural shapes we emit)
 * - `isUnit` — true for void/undefined returns (→ WIT `output-schema.unit`);
 *              the graph/codec are placeholders in that case
 * - `codec`  — `Codec<domainType, SchemaValue>`, composed on top of the user's
 *              own schema so refinements/transformations are honoured
 *
 * @since 1.6.0
 * @category codecs
 */
export interface WitCodec<S extends Schema.Top> {
  readonly schema: S
  readonly graph: SchemaGraph
  readonly isUnit: boolean
  readonly codec: Schema.Codec<S["Type"], SchemaValue, S["DecodingServices"], S["EncodingServices"]>
}

/** A schema-native compiler result with direct sync and async wire conversion. */
export interface CompiledWitCodec<S extends Schema.Top> extends WitCodec<S> {
  readonly schemaGraph: CoreTypes.SchemaGraph
  readonly encode: (
    value: S["Type"],
  ) => Effect.Effect<CoreTypes.SchemaValueTree, Schema.SchemaError, S["EncodingServices"]>
  readonly encodeAsync: (
    value: S["Type"],
  ) => Effect.Effect<CoreTypes.SchemaValueTree, Schema.SchemaError, S["EncodingServices"]>
  readonly decode: (
    value: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<S["Type"], Schema.SchemaError, S["DecodingServices"]>
}

/** Leaf pair for a primitive whose schema value carries a single `value`. */
const primPair = (make: (val: any) => SchemaValue): ValuePair => ({
  toValue: (val) => make(val),
  fromValue: (sv) => (sv as { value: unknown }).value,
})

const assertValueShape = (
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
  path = "value",
): void => {
  const body = type.body.tag === "ref" ? graph.defs.get(type.body.id)?.body.body : type.body
  if (body === undefined) throw new Error(`${path}: unresolved schema reference`)
  const expectedTag = body.tag === "fixed-list" ? "fixed-list" : body.tag
  if (value.tag !== expectedTag) {
    throw new Error(`${path}: expected ${expectedTag}, received ${value.tag}`)
  }
  switch (body.tag) {
    case "record":
      if (value.tag !== "record" || value.fields.length !== body.fields.length) {
        throw new Error(`${path}: expected record with ${body.fields.length} fields`)
      }
      body.fields.forEach((field, i) =>
        assertValueShape(graph, field.body, value.fields[i]!, `${path}.${field.name}`),
      )
      break
    case "tuple":
      if (value.tag !== "tuple" || value.elements.length !== body.elements.length) {
        throw new Error(`${path}: expected tuple with ${body.elements.length} elements`)
      }
      body.elements.forEach((element, i) =>
        assertValueShape(graph, element, value.elements[i]!, `${path}[${i}]`),
      )
      break
    case "list":
      if (value.tag === "list") {
        value.elements.forEach((element, i) =>
          assertValueShape(graph, body.element, element, `${path}[${i}]`),
        )
      }
      break
    case "fixed-list":
      if (value.tag !== "fixed-list" || value.elements.length !== body.length) {
        throw new Error(`${path}: expected fixed-list with ${body.length} elements`)
      }
      value.elements.forEach((element, i) =>
        assertValueShape(graph, body.element, element, `${path}[${i}]`),
      )
      break
    case "option":
      if (value.tag === "option" && value.value !== undefined) {
        assertValueShape(graph, body.element, value.value, `${path}.some`)
      }
      break
    case "variant": {
      if (value.tag !== "variant") break
      const variant = body.cases[value.caseIndex]
      if (variant === undefined) throw new Error(`${path}: invalid variant case ${value.caseIndex}`)
      if ((variant.payload === undefined) !== (value.payload === undefined)) {
        throw new Error(`${path}: variant payload arity mismatch`)
      }
      if (variant.payload !== undefined && value.payload !== undefined) {
        assertValueShape(graph, variant.payload, value.payload, `${path}.${variant.name}`)
      }
      break
    }
    case "enum":
      if (value.tag === "enum" && body.cases[value.caseIndex] === undefined) {
        throw new Error(`${path}: invalid enum case ${value.caseIndex}`)
      }
      break
    case "flags":
      if (value.tag !== "flags" || value.flags.length !== body.names.length) {
        throw new Error(`${path}: expected flags with ${body.names.length} values`)
      }
      break
    case "union": {
      if (value.tag !== "union") break
      const branch = body.branches.find((candidate) => candidate.tag === value.unionTag)
      if (branch === undefined) throw new Error(`${path}: unknown union branch '${value.unionTag}'`)
      assertValueShape(graph, branch.body, value.body, `${path}.${branch.tag}`)
      break
    }
  }
}

/**
 * Mapping from a {@link WitNumericKind} annotation to the matching schema-type
 * constructor, schema-value constructor, and a coercion re-shaping the encoded
 * JS value into what the value expects (e.g. `bigint` for `u64`/`s64`).
 */
const numericMapping: Record<
  WitNumericKind,
  {
    make: (r?: NumericRestrictions) => SchemaType
    toV: (v: any) => SchemaValue
    coerce: (v: any) => any
  }
> = {
  u8: { make: t.u8, toV: v.u8, coerce: (v) => v },
  u16: { make: t.u16, toV: v.u16, coerce: (v) => v },
  u32: { make: t.u32, toV: v.u32, coerce: (v) => v },
  u64: {
    make: t.u64,
    toV: v.u64,
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  s8: { make: t.s8, toV: v.s8, coerce: (v) => v },
  s16: { make: t.s16, toV: v.s16, coerce: (v) => v },
  s32: { make: t.s32, toV: v.s32, coerce: (v) => v },
  s64: {
    make: t.s64,
    toV: v.s64,
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  f32: { make: t.f32, toV: v.f32, coerce: (v) => v },
  f64: { make: t.f64, toV: v.f64, coerce: (v) => v },
}

/**
 * Look up an annotation by key on an AST node. Effect Schema attaches
 * annotations from `Schema.annotate(...)` to the *last check* (refinement),
 * not the AST root, so we delegate to `SchemaAST.resolveAt` which knows the
 * right traversal order.
 */
const annotationOf = <T = unknown>(a: SchemaAST.AST, key: string): T | undefined =>
  (
    SchemaAST as unknown as {
      resolveAt: <U>(k: string) => (a: SchemaAST.AST) => U | undefined
    }
  ).resolveAt<T>(key)(a)

const metadataOf = (a: SchemaAST.AST): MetadataEnvelope => {
  const annotations = (a.annotations ?? {}) as Record<string, unknown>
  const explicit = annotationOf<Partial<MetadataEnvelope>>(a, witMetadataAnnotationKey) ?? {}
  const examples =
    explicit.examples ??
    (Array.isArray(annotations.examples)
      ? annotations.examples.map((example) => JSON.stringify(example))
      : [])
  return {
    ...emptyMetadata(),
    ...(typeof annotations.description === "string" ? { doc: annotations.description } : {}),
    ...(typeof annotations.deprecated === "string" ? { deprecated: annotations.deprecated } : {}),
    ...explicit,
    aliases: explicit.aliases ? [...explicit.aliases] : [],
    examples: [...examples],
  }
}

const numericKindOf = (a: SchemaAST.AST): WitNumericKind | undefined =>
  annotationOf<WitNumericKind>(a, witTypeAnnotationKey)

const F64_BITS_VIEW = new DataView(new ArrayBuffer(8))
const f64Bits = (x: number): bigint => {
  // Canonicalize -0.0 to +0.0 so equal bounds compare equal (mirrors the codec).
  F64_BITS_VIEW.setFloat64(0, x === 0 ? 0 : x)
  return F64_BITS_VIEW.getBigUint64(0)
}

/** The `numeric-bound` tag for a numeric pin kind. */
const boundKindOf = (kind: WitNumericKind): "signed" | "unsigned" | "float-bits" =>
  kind[0] === "f" ? "float-bits" : kind[0] === "s" ? "signed" : "unsigned"

const makeBound = (
  boundKind: "signed" | "unsigned" | "float-bits",
  x: number | bigint,
): NumericBound =>
  boundKind === "float-bits"
    ? { tag: "float-bits", val: f64Bits(Number(x)) }
    : { tag: boundKind, val: BigInt(x) }

/** Read inline numeric restrictions (from a `restrict(...)` annotation) for `kind`. */
const numericRestrictionsOf = (
  a: SchemaAST.AST,
  kind: WitNumericKind,
): NumericRestrictions | undefined => {
  const opts = annotationOf<NumericRestrictionsInput>(a, witNumericRestrictionsKey)
  if (!opts || (opts.min === undefined && opts.max === undefined && !opts.unit)) return undefined
  const bk = boundKindOf(kind)
  return {
    min: opts.min !== undefined ? makeBound(bk, opts.min) : undefined,
    max: opts.max !== undefined ? makeBound(bk, opts.max) : undefined,
    unit: opts.unit,
  }
}

const numericNode = (
  kind: WitNumericKind,
  a: SchemaAST.AST,
): { type: SchemaType; pair: ValuePair } => {
  const m = numericMapping[kind]
  return {
    type: m.make(numericRestrictionsOf(a, kind)),
    pair: {
      toValue: (val) => m.toV(m.coerce(val)),
      fromValue: (sv) => (sv as { value: unknown }).value,
    },
  }
}

const isNullOrUndefinedAST = (a: SchemaAST.AST): boolean =>
  a._tag === "Null" || a._tag === "Undefined" || a._tag === "Void"

const isVoidLikeAST = (a: SchemaAST.AST): boolean => a._tag === "Undefined" || a._tag === "Void"

/**
 * A "shape signature" classifying an AST's *encoded* form, used to dispatch
 * encoded values to the matching variant case at encode time and to reject
 * unions whose members would be ambiguous.
 */
interface EncodedShape {
  readonly tag:
    | "string"
    | "number"
    | "boolean"
    | "bigint"
    | "null"
    | "undefined"
    | "literal"
    | "array"
    | "object"
    | "object-with-tag"
    | "unknown"
  readonly literal?: string | number | boolean | bigint
  /** For `object-with-tag`: the discriminator value of the `_tag` field. */
  readonly tagLiteral?: string
  readonly matches: (v: unknown) => boolean
}

/**
 * Compute an `EncodedShape` for a schema AST. Operates on the *encoded* form
 * because the codec walks encoded values, not decoded ones.
 */
const encodedShapeOf = (a: SchemaAST.AST): EncodedShape => {
  switch (a._tag) {
    case "String":
      return { tag: "string", matches: (v) => typeof v === "string" }
    case "Number":
      return { tag: "number", matches: (v) => typeof v === "number" }
    case "Boolean":
      return { tag: "boolean", matches: (v) => typeof v === "boolean" }
    case "BigInt":
      return { tag: "bigint", matches: (v) => typeof v === "bigint" }
    case "Null":
      return { tag: "null", matches: (v) => v === null }
    case "Undefined":
    case "Void":
      return { tag: "undefined", matches: (v) => v === undefined }
    case "Literal": {
      const lit = (a as SchemaAST.Literal).literal
      return {
        tag: "literal",
        literal: lit as EncodedShape["literal"],
        matches: (v) => v === lit,
      }
    }
    case "Arrays":
      return { tag: "array", matches: Array.isArray }
    case "Enum": {
      const values = (a as SchemaAST.Enum).enums.map(([, value]) => value)
      const kind = typeof values[0]
      return {
        tag: kind === "string" ? "string" : "number",
        matches: (value) => typeof value === kind && values.includes(value as never),
      }
    }
    case "Objects": {
      const tagPs = (a as SchemaAST.Objects).propertySignatures.find((ps) => ps.name === "_tag")
      // Only treat _tag as a discriminator if the property is *required*.
      if (
        tagPs !== undefined &&
        !SchemaAST.isOptional(tagPs.type) &&
        SchemaAST.isLiteral(tagPs.type) &&
        typeof tagPs.type.literal === "string"
      ) {
        const lit = tagPs.type.literal as string
        return {
          tag: "object-with-tag",
          tagLiteral: lit,
          matches: (v) =>
            typeof v === "object" &&
            v !== null &&
            !Array.isArray(v) &&
            (v as { _tag?: unknown })._tag === lit,
        }
      }
      return {
        tag: "object",
        matches: (v) => typeof v === "object" && v !== null && !Array.isArray(v),
      }
    }
    default:
      return { tag: "unknown", matches: () => true }
  }
}

const variantCaseNameOf = (a: SchemaAST.AST): string | undefined =>
  annotationOf<string>(a, variantCaseNameAnnotationKey)

const discriminatorMatches = (rule: UnionBranch["discriminator"], value: unknown): boolean => {
  switch (rule.tag) {
    case "prefix":
      return typeof value === "string" && value.startsWith(rule.val)
    case "suffix":
      return typeof value === "string" && value.endsWith(rule.val)
    case "contains":
      return typeof value === "string" && value.includes(rule.val)
    case "regex":
      return typeof value === "string" && new RegExp(rule.val, "u").test(value)
    case "field-equals":
      return (
        typeof value === "object" &&
        value !== null &&
        rule.val.fieldName in value &&
        (rule.val.literal === undefined ||
          (value as Record<string, unknown>)[rule.val.fieldName] === rule.val.literal)
      )
    case "field-absent":
      return typeof value === "object" && value !== null && !(rule.val in value)
  }
}

const declarationConstructorTag = (a: SchemaAST.AST): string | undefined => {
  if (a._tag !== "Declaration") return undefined
  const tc = (a.annotations as { typeConstructor?: { _tag?: string } } | undefined)?.typeConstructor
  return tc?._tag
}

const typedArrayKindOf = (a: SchemaAST.AST): WitTypedArrayKind | undefined =>
  annotationOf<WitTypedArrayKind>(a, witTypedArrayAnnotationKey)

const isQuotaTokenAST = (a: SchemaAST.AST): boolean =>
  annotationOf<boolean>(a, witQuotaTokenAnnotationKey) === true

const isPrincipalAST = (a: SchemaAST.AST): boolean =>
  annotationOf<boolean>(a, witPrincipalAnnotationKey) === true

// ---------- Principal (WIT `golem:agent/common` `principal` variant) ----------
//
// A `Principal` carried as ordinary structured data (a method param / return),
// as opposed to the `Principal` capability service. The graph mirrors the host
// `Principal` shape exactly (case order oidc/agent/golem-user/anonymous), and
// the value pair round-trips a host `Principal` <-> `SchemaValue`.

type HostUuid = { highBits: bigint; lowBits: bigint }

// --- graph type builders (no recursion: everything is built inline) ---
const uuidType = (): SchemaType => t.record([field("highBits", t.u64()), field("lowBits", t.u64())])
const componentIdType = (): SchemaType => t.record([field("uuid", uuidType())])
const agentIdType = (): SchemaType =>
  t.record([field("componentId", componentIdType()), field("agentId", t.string())])
const accountIdType = (): SchemaType => t.record([field("uuid", uuidType())])
const oidcType = (): SchemaType =>
  t.record([
    field("sub", t.string()),
    field("issuer", t.string()),
    field("email", t.option(t.string())),
    field("name", t.option(t.string())),
    field("emailVerified", t.option(t.bool())),
    field("givenName", t.option(t.string())),
    field("familyName", t.option(t.string())),
    field("picture", t.option(t.string())),
    field("preferredUsername", t.option(t.string())),
    field("claims", t.string()),
  ])
const agentPrincipalType = (): SchemaType => t.record([field("agentId", agentIdType())])
const golemUserType = (): SchemaType => t.record([field("accountId", accountIdType())])

// --- SchemaValue field accessors (positional record reads) ---
const recFields = (sv: SchemaValue): ReadonlyArray<SchemaValue> =>
  (sv as { tag: "record"; fields: ReadonlyArray<SchemaValue> }).fields
const u64Of = (f: SchemaValue): bigint => (f as { tag: "u64"; value: bigint }).value
const strOf = (f: SchemaValue): string => (f as { tag: "string"; value: string }).value
const boolOf = (f: SchemaValue): boolean => (f as { tag: "bool"; value: boolean }).value
const optOf = (f: SchemaValue): SchemaValue | undefined =>
  (f as { tag: "option"; value?: SchemaValue }).value

// --- codec helpers (round-trip via the host shape) ---
const uuidToValue = (u: HostUuid): SchemaValue => v.record([v.u64(u.highBits), v.u64(u.lowBits)])
const uuidFromValue = (sv: SchemaValue): HostUuid => {
  const f = recFields(sv)
  return { highBits: u64Of(f[0]!), lowBits: u64Of(f[1]!) }
}

const agentIdToValue = (a: AgentCommon.AgentId): SchemaValue =>
  v.record([v.record([uuidToValue(a.componentId.uuid)]), v.string(a.agentId)])
const agentIdFromValue = (sv: SchemaValue): AgentCommon.AgentId => {
  const f = recFields(sv)
  const uuid = uuidFromValue(recFields(f[0]!)[0]!)
  return { componentId: { uuid }, agentId: strOf(f[1]!) }
}

const accountIdToValue = (a: AgentCommon.AccountId): SchemaValue => v.record([uuidToValue(a.uuid)])
const accountIdFromValue = (sv: SchemaValue): AgentCommon.AccountId => ({
  uuid: uuidFromValue(recFields(sv)[0]!),
})

const oidcToValue = (o: AgentCommon.OidcPrincipal): SchemaValue => {
  const optStr = (x: string | undefined): SchemaValue =>
    v.option(x === undefined ? undefined : v.string(x))
  return v.record([
    v.string(o.sub),
    v.string(o.issuer),
    optStr(o.email),
    optStr(o.name),
    v.option(o.emailVerified === undefined ? undefined : v.bool(o.emailVerified)),
    optStr(o.givenName),
    optStr(o.familyName),
    optStr(o.picture),
    optStr(o.preferredUsername),
    v.string(o.claims),
  ])
}
const oidcFromValue = (sv: SchemaValue): AgentCommon.OidcPrincipal => {
  const f = recFields(sv)
  const optStr = (fld: SchemaValue): string | undefined => {
    const val = optOf(fld)
    return val === undefined ? undefined : strOf(val)
  }
  const out: AgentCommon.OidcPrincipal = {
    sub: strOf(f[0]!),
    issuer: strOf(f[1]!),
    claims: strOf(f[9]!),
  }
  const email = optStr(f[2]!)
  if (email !== undefined) out.email = email
  const name = optStr(f[3]!)
  if (name !== undefined) out.name = name
  const ev = optOf(f[4]!)
  if (ev !== undefined) out.emailVerified = boolOf(ev)
  const givenName = optStr(f[5]!)
  if (givenName !== undefined) out.givenName = givenName
  const familyName = optStr(f[6]!)
  if (familyName !== undefined) out.familyName = familyName
  const picture = optStr(f[7]!)
  if (picture !== undefined) out.picture = picture
  const preferredUsername = optStr(f[8]!)
  if (preferredUsername !== undefined) out.preferredUsername = preferredUsername
  return out
}

/**
 * Type + value bridge for the `principal` data variant. The graph root mirrors
 * the host `Principal` shape (case order oidc/agent/golem-user/anonymous); the
 * value pair lowers a host `Principal` into the matching `v.variant(...)` and
 * lifts it back out.
 */
const principalNode = (): { type: SchemaType; pair: ValuePair } => ({
  type: t.variant([
    variantCase("oidc", oidcType()),
    variantCase("agent", agentPrincipalType()),
    variantCase("golem-user", golemUserType()),
    variantCase("anonymous"),
  ]),
  pair: {
    toValue: (p: AgentCommon.Principal) => {
      switch (p.tag) {
        case "oidc":
          return v.variant(0, oidcToValue(p.val))
        case "agent":
          return v.variant(1, v.record([agentIdToValue(p.val.agentId)]))
        case "golem-user":
          return v.variant(2, v.record([accountIdToValue(p.val.accountId)]))
        case "anonymous":
          return v.variant(3)
      }
    },
    fromValue: (sv): AgentCommon.Principal => {
      const vv = sv as { caseIndex: number; payload?: SchemaValue }
      switch (vv.caseIndex) {
        case 0:
          return { tag: "oidc", val: oidcFromValue(vv.payload as SchemaValue) }
        case 1:
          return {
            tag: "agent",
            val: { agentId: agentIdFromValue(recFields(vv.payload as SchemaValue)[0]!) },
          }
        case 2:
          return {
            tag: "golem-user",
            val: { accountId: accountIdFromValue(recFields(vv.payload as SchemaValue)[0]!) },
          }
        default:
          return { tag: "anonymous" }
      }
    },
  },
})

/**
 * Type + value bridge for the opaque `quota-token` capability node. The graph
 * root is `t.quotaToken({})`; the value pair lowers a host `QuotaToken` (a raw
 * owned `own<quota-token>` resource) into `v.quotaToken(handle)` and lifts it
 * back out via `handle.take()`. The take-once cell guarantees the owned handle
 * is moved exactly once; decoding a value whose handle was already consumed
 * throws.
 */
const quotaTokenNode = (): { type: SchemaType; pair: ValuePair } => ({
  type: t.quotaToken({}),
  pair: {
    toValue: (handle) => {
      if (!(handle instanceof GuestQuotaTokenHandle)) {
        throw new Error("quota-token schemas only accept SDK-owned quota tokens")
      }
      return v.quotaToken(handle)
    },
    fromValue: (sv) => {
      const handle = (sv as { handle: GuestQuotaTokenHandle }).handle
      const raw = peekGuestQuotaTokenHandle(QUOTA_INTERNAL, handle)
      if (raw === undefined) {
        throw new Error(
          "quota-token handle was already consumed; an owned quota-token can only be decoded once",
        )
      }
      return handle
    },
  },
})

/**
 * Per-typed-array element schema-type/value constructors plus an optional
 * coercion (used to keep `bigint` payloads for s64/u64 arrays without forcing
 * the user to pre-convert).
 */
const typedArrayElement: Record<
  WitTypedArrayKind,
  { make: () => SchemaType; toV: (v: any) => SchemaValue; coerce: (v: unknown) => any }
> = {
  u8: { make: t.u8, toV: v.u8, coerce: (v) => v },
  i8: { make: t.s8, toV: v.s8, coerce: (v) => v },
  u16: { make: t.u16, toV: v.u16, coerce: (v) => v },
  i16: { make: t.s16, toV: v.s16, coerce: (v) => v },
  u32: { make: t.u32, toV: v.u32, coerce: (v) => v },
  i32: { make: t.s32, toV: v.s32, coerce: (v) => v },
  f32: { make: t.f32, toV: v.f32, coerce: (v) => v },
  f64: { make: t.f64, toV: v.f64, coerce: (v) => v },
  "big-i64": {
    make: t.s64,
    toV: v.s64,
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
  "big-u64": {
    make: t.u64,
    toV: v.u64,
    coerce: (v) => (typeof v === "bigint" ? v : BigInt(v as number)),
  },
}

/** Construct the JS TypedArray subclass matching a `WitTypedArrayKind`. */
const typedArrayCtor: Record<WitTypedArrayKind, new (entries: Iterable<any>) => any> = {
  u8: Uint8Array,
  i8: Int8Array,
  u16: Uint16Array,
  i16: Int16Array,
  u32: Uint32Array,
  i32: Int32Array,
  f32: Float32Array,
  f64: Float64Array,
  "big-i64": BigInt64Array,
  "big-u64": BigUint64Array,
}

/**
 * Walk a Schema AST into a recursive {@link SchemaType} plus a top-level
 * {@link ValuePair}. Unlike the WIT-graph model, the type is recursive — the
 * flattening into the indexed `schema-graph` is done later by `GraphEncoder`.
 */
const walk = (
  ast: SchemaAST.AST,
): Effect.Effect<
  { type: SchemaType; pair: ValuePair; defs: SchemaGraph["defs"] },
  UnsupportedSchemaError
> => {
  interface Entry {
    readonly id: string
    active: boolean
    recursive: boolean
    pair?: ValuePair
  }
  const entries = new WeakMap<object, Entry>()
  const defs = new Map<string, { name?: string; body: SchemaType }>()
  let nextId = 0
  return Effect.gen(function* () {
    const unsupported = (reason: string) => Effect.fail(new UnsupportedSchemaError(reason))

    /** Recurse into a child AST, returning its type + pair. */
    const child = (
      a: SchemaAST.AST,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      nodeFor(a, true)

    /**
     * Build a record-type node + pair given a list of property signatures.
     * Used for both `Objects` schemas and tagged-variant payloads.
     */
    const recordNode = (
      props: ReadonlyArray<SchemaAST.PropertySignature>,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        type Field = {
          readonly name: string
          readonly type: SchemaType
          readonly pair: ValuePair
          readonly optional: boolean
        }
        const fields: Array<Field> = []
        for (const ps of props) {
          if (typeof ps.name !== "string") {
            return yield* unsupported(`non-string property key: ${String(ps.name)}`)
          }
          const { type: rawType, pair: rawPair } = yield* child(ps.type)
          const optional = SchemaAST.isOptional(ps.type)
          if (optional) {
            const pair: ValuePair = {
              toValue: (val) => v.option(val === undefined ? undefined : rawPair.toValue(val)),
              fromValue: (sv) => {
                const ov = sv as { value?: SchemaValue }
                return ov.value === undefined ? undefined : rawPair.fromValue(ov.value)
              },
            }
            fields.push({ name: ps.name, type: t.option(rawType), pair, optional })
          } else {
            fields.push({ name: ps.name, type: rawType, pair: rawPair, optional })
          }
        }
        const type = t.record(fields.map((f) => field(f.name, f.type, f.type.metadata)))
        const pair: ValuePair = {
          toValue: (obj: Record<string, unknown>) =>
            v.record(fields.map((f) => f.pair.toValue(obj[f.name]))),
          fromValue: (sv) => {
            const rv = sv as { fields: ReadonlyArray<SchemaValue> }
            const out: Record<string, unknown> = {}
            for (let i = 0; i < fields.length; i++) {
              const f = fields[i]!
              const val = f.pair.fromValue(rv.fields[i]!)
              if (!f.optional || val !== undefined) out[f.name] = val
            }
            return out
          },
        }
        return { type, pair }
      })

    /**
     * Build an option-type node + pair wrapping an inner type/pair, with a
     * configurable "encoded empty" representation (the value standing in for
     * `None` in the user's encoded form, e.g. `null` for `NullOr`).
     */
    const optionWrap = (
      innerType: SchemaType,
      innerPair: ValuePair,
      empty: { readonly kind: "null" | "undefined" | "effect-option" },
    ): { type: SchemaType; pair: ValuePair } => {
      const isEmpty = (val: unknown): boolean => {
        switch (empty.kind) {
          case "null":
            return val === null
          case "undefined":
            return val === undefined
          case "effect-option":
            return Option.isOption(val as any) && Option.isNone(val as any)
        }
      }
      const wrap = (encoded: unknown): unknown =>
        empty.kind === "effect-option" ? Option.some(encoded) : encoded
      const unwrap = (val: any): unknown =>
        empty.kind === "effect-option"
          ? (val as Option.Option<unknown>).pipe(Option.getOrElse(() => undefined))
          : val
      const emptyEncoded = (): unknown => {
        switch (empty.kind) {
          case "null":
            return null
          case "undefined":
            return undefined
          case "effect-option":
            return Option.none()
        }
      }
      const pair: ValuePair = {
        toValue: (val) => v.option(isEmpty(val) ? undefined : innerPair.toValue(unwrap(val))),
        fromValue: (sv) => {
          const ov = sv as { value?: SchemaValue }
          if (ov.value === undefined) return emptyEncoded()
          return wrap(innerPair.fromValue(ov.value))
        },
      }
      return { type: t.option(innerType), pair }
    }

    /** Build a type + pair for a single AST node (no graph mutation). */
    const buildNode = (
      a: SchemaAST.AST,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const native = annotationOf<WitSchemaNode>(a, witSchemaNodeAnnotationKey)
        if (native !== undefined) {
          switch (native.tag) {
            case "text":
              return {
                type: schemaType(native),
                pair: {
                  toValue: v.text,
                  fromValue: (x) => (x as Extract<SchemaValue, { tag: "text" }>).text,
                },
              }
            case "binary":
              return {
                type: schemaType(native),
                pair: {
                  toValue: v.binary,
                  fromValue: (x) => (x as Extract<SchemaValue, { tag: "binary" }>).bytes,
                },
              }
            case "path":
              return { type: t.path(native.spec), pair: primPair(v.path) }
            case "url":
              return { type: t.url(native.restrictions), pair: primPair(v.url) }
            case "datetime":
              return { type: t.datetime(), pair: primPair(v.datetime) }
            case "duration":
              return {
                type: t.duration(),
                pair: {
                  toValue: v.duration,
                  fromValue: (x) => (x as Extract<SchemaValue, { tag: "duration" }>).nanoseconds,
                },
              }
            case "quantity":
              return { type: t.quantity(native.spec), pair: primPair(v.quantity) }
            case "fixed-list": {
              if (a._tag !== "Arrays" || a.rest.length !== 1) {
                return yield* unsupported("FixedList annotation must be attached to Schema.Array")
              }
              const item = yield* child(a.rest[0]!)
              return {
                type: t.fixedList(item.type, native.length),
                pair: {
                  toValue: (xs: ReadonlyArray<unknown>) => v.fixedList(xs.map(item.pair.toValue)),
                  fromValue: (x) =>
                    (x as Extract<SchemaValue, { tag: "fixed-list" }>).elements.map(
                      item.pair.fromValue,
                    ),
                },
              }
            }
            case "map": {
              if (a._tag !== "Declaration" || a.typeParameters.length < 2) {
                return yield* unsupported("Map annotation must be attached to Schema.ReadonlyMap")
              }
              const key = yield* child(a.typeParameters[0]!)
              const value = yield* child(a.typeParameters[1]!)
              return {
                type: t.map(key.type, value.type),
                pair: {
                  toValue: (map: ReadonlyMap<unknown, unknown>) =>
                    v.map(
                      [...map].map(([k, val]) => ({
                        key: key.pair.toValue(k),
                        value: value.pair.toValue(val),
                      })),
                    ),
                  fromValue: (x) =>
                    new Map(
                      (x as Extract<SchemaValue, { tag: "map" }>).entries.map((entry) => [
                        key.pair.fromValue(entry.key),
                        value.pair.fromValue(entry.value),
                      ]),
                    ),
                },
              }
            }
            case "flags": {
              const duplicate = native.names.find((name, i) => native.names.indexOf(name) !== i)
              if (native.names.length === 0) return yield* unsupported("flags has no names")
              if (duplicate !== undefined) {
                return yield* unsupported(`duplicate flag name '${duplicate}'`)
              }
              return {
                type: t.flags([...native.names]),
                pair: {
                  toValue: (flags: ReadonlyArray<boolean>) => v.flags([...flags]),
                  fromValue: (value) => [
                    ...(value as Extract<SchemaValue, { tag: "flags" }>).flags,
                  ],
                },
              }
            }
            case "discriminated-union": {
              const branches: Array<{
                model: UnionBranch
                pair: ValuePair
              }> = []
              for (const branch of native.branches) {
                const compiled = yield* child(SchemaAST.toEncoded(branch.schema.ast))
                branches.push({
                  model: {
                    tag: branch.tag,
                    body: compiled.type,
                    discriminator: branch.discriminator,
                    metadata: compiled.type.metadata,
                  },
                  pair: compiled.pair,
                })
              }
              const type = t.union(branches.map((branch) => branch.model))
              const errors = validateSchemaGraph({ defs, root: type })
              if (errors.length > 0) {
                return yield* unsupported(errors.map((error) => error.message).join("; "))
              }
              return {
                type,
                pair: {
                  toValue: (value) => {
                    const matches = branches.filter((branch) =>
                      discriminatorMatches(branch.model.discriminator, value),
                    )
                    if (matches.length !== 1) {
                      throw new Error(
                        `discriminated union value matched ${matches.length} branches; expected exactly one`,
                      )
                    }
                    const branch = matches[0]!
                    return v.union(branch.model.tag, branch.pair.toValue(value))
                  },
                  fromValue: (value) => {
                    const union = value as Extract<SchemaValue, { tag: "union" }>
                    const branch = branches.find((entry) => entry.model.tag === union.unionTag)
                    if (branch === undefined) {
                      throw new Error(`unknown discriminated union tag '${union.unionTag}'`)
                    }
                    const decoded = branch.pair.fromValue(union.body)
                    if (!discriminatorMatches(branch.model.discriminator, decoded)) {
                      throw new Error(
                        `discriminated union branch '${branch.model.tag}' does not match its discriminator`,
                      )
                    }
                    return decoded
                  },
                },
              }
            }
            case "secret": {
              if (a._tag !== "Declaration" || a.typeParameters[0] === undefined) {
                return yield* unsupported("Secret requires an inner schema")
              }
              const inner = yield* child(a.typeParameters[0])
              return {
                type: t.secret(inner.type, { category: native.category }),
                pair: {
                  toValue: (handle: GuestSecretHandle) => v.secret(handle),
                  fromValue: (x) => {
                    const handle = (x as Extract<SchemaValue, { tag: "secret" }>).handle
                    if (peekGuestSecretHandle(SECRET_INTERNAL, handle) === undefined) {
                      throw new Error("secret handle was already consumed")
                    }
                    return handle
                  },
                },
              }
            }
            case "quota-token":
              return {
                type: t.quotaToken({ resourceName: native.resourceName }),
                pair: quotaTokenNode().pair,
              }
            case "permission-card":
              return {
                type: t.permissionCard(native.spec),
                pair: {
                  toValue: (handle: GuestPermissionCardHandle) => v.permissionCard(handle),
                  fromValue: (x) => {
                    const handle = (x as Extract<SchemaValue, { tag: "permission-card" }>).handle
                    if (
                      peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handle) === undefined
                    ) {
                      throw new Error("permission-card handle was already consumed")
                    }
                    return handle
                  },
                },
              }
            case "stream": {
              if (a._tag !== "Declaration" || a.typeParameters[0] === undefined) {
                return yield* unsupported("AgentStream requires an element schema")
              }
              const itemAst = a.typeParameters[0]
              const item = yield* child(itemAst)
              const itemSchema = native.elementSchema ?? (Schema.make(itemAst) as Schema.Top)
              let itemCodec: WitCodec<Schema.Top> | undefined
              const codec = () => (itemCodec ??= Effect.runSync(toWitCodec(itemSchema))).codec
              return {
                type: t.stream(item.type),
                pair: {
                  toValue: (stream: AgentStream<unknown>) =>
                    v.stream(agentStreamToHandle(stream, codec(), requireConversionContext())),
                  fromValue: (x) =>
                    agentStreamFromHandle(
                      (x as Extract<SchemaValue, { tag: "stream" }>).handle,
                      codec(),
                      requireConversionContext(),
                    ),
                },
              }
            }
          }
        }
        switch (a._tag) {
          case "String": {
            // `Char` annotates Schema.Char with witType: "char".
            const witHint = annotationOf<string>(a, witTypeAnnotationKey)
            if (witHint === "char") {
              return { type: t.char(), pair: primPair(v.char) }
            }
            return { type: t.string(), pair: primPair(v.string) }
          }
          case "Boolean":
            return { type: t.bool(), pair: primPair(v.bool) }
          case "Literal": {
            // Preserve the received payload so Effect Schema validates the literal.
            const lit = (a as SchemaAST.Literal).literal
            if (typeof lit === "string") {
              return {
                type: t.string(),
                pair: primPair(v.string),
              }
            }
            if (typeof lit === "boolean") {
              return { type: t.bool(), pair: primPair(v.bool) }
            }
            if (typeof lit === "number") {
              return { type: t.f64(), pair: primPair(v.f64) }
            }
            if (typeof lit === "bigint") {
              return { type: t.s64(), pair: primPair(v.s64) }
            }
            return yield* unsupported(`unsupported literal type: ${typeof lit}`)
          }
          case "Number": {
            const kind = numericKindOf(a) ?? "f64"
            return numericNode(kind, a)
          }
          case "BigInt": {
            const kind = numericKindOf(a) ?? "s64"
            return numericNode(kind, a)
          }
          case "Enum": {
            if (a.enums.length === 0) return yield* unsupported("empty enum")
            const allStrings = a.enums.every(([, value]) => typeof value === "string")
            const cases = allStrings
              ? a.enums.map(([, value]) => value as string)
              : a.enums.map(([name]) => name)
            const values = a.enums.map(([, value]) => value)
            return {
              type: t.enum(cases),
              pair: {
                toValue: (value) => v.enum(values.indexOf(value)),
                fromValue: (value) => values[(value as { caseIndex: number }).caseIndex],
              },
            }
          }

          case "Objects": {
            if (a.indexSignatures.length > 0) {
              return yield* unsupported(
                "index signatures cannot be represented in the schema model",
              )
            }
            return yield* recordNode(a.propertySignatures)
          }

          case "Arrays": {
            if (a.rest.length === 0 && a.elements.length > 0) {
              const elemTypes: Array<SchemaType> = []
              const elemPairs: Array<ValuePair> = []
              for (const el of a.elements) {
                const { type, pair } = yield* child(el)
                elemTypes.push(type)
                elemPairs.push(pair)
              }
              return {
                type: t.tuple(elemTypes),
                pair: {
                  toValue: (arr: ReadonlyArray<unknown>) =>
                    v.tuple(elemPairs.map((p, i) => p.toValue(arr[i]))),
                  fromValue: (sv) => {
                    const tv = sv as { elements: ReadonlyArray<SchemaValue> }
                    return elemPairs.map((p, i) => p.fromValue(tv.elements[i]!))
                  },
                },
              }
            }
            if (a.elements.length === 0 && a.rest.length === 1) {
              const { type, pair } = yield* child(a.rest[0]!)
              return {
                type: t.list(type),
                pair: {
                  toValue: (arr: ReadonlyArray<unknown>) => v.list(arr.map((x) => pair.toValue(x))),
                  fromValue: (sv) => {
                    const lv = sv as { elements: ReadonlyArray<SchemaValue> }
                    return lv.elements.map((c) => pair.fromValue(c))
                  },
                },
              }
            }
            return yield* unsupported("mixed tuple/rest arrays are not supported")
          }

          case "Union":
            return yield* unionNode(a)

          case "Declaration": {
            // The opaque `quota-token` capability is a declared schema marked
            // with `witQuotaTokenAnnotationKey`; emit the dedicated capability
            // node rather than treating it as an unknown declared type.
            if (isQuotaTokenAST(a)) {
              return quotaTokenNode()
            }
            // A `Principal` carried as data (annotated via `PrincipalSchema`):
            // emit the `principal` variant rather than an unknown declared type.
            if (isPrincipalAST(a)) {
              return principalNode()
            }
            // Typed-array hints (Uint8ArraySchema, …) take precedence — emit a
            // dedicated `list<primN>` shape rather than an unknown declared type.
            const tak = typedArrayKindOf(a)
            if (tak !== undefined) {
              const elem = typedArrayElement[tak]
              const Ctor = typedArrayCtor[tak]
              return {
                type: t.list(elem.make()),
                pair: {
                  toValue: (arr) => {
                    const items: Array<SchemaValue> = []
                    for (const x of arr as Iterable<unknown>) items.push(elem.toV(elem.coerce(x)))
                    return v.list(items)
                  },
                  fromValue: (sv) => {
                    const lv = sv as { elements: ReadonlyArray<SchemaValue> }
                    const raw = lv.elements.map((c) => (c as { value: unknown }).value)
                    return new Ctor(raw as Iterable<any>)
                  },
                },
              }
            }
            return yield* declarationNode(a)
          }

          default:
            return yield* unsupported(`unsupported AST node: ${a._tag}`)
        }
      })

    const nodeFor = (
      raw: SchemaAST.AST,
      alreadyEncoded = false,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        if (raw._tag === "Suspend") {
          const thunk = (raw as unknown as { thunk: () => SchemaAST.AST }).thunk
          return yield* nodeFor(thunk(), true)
        }
        const key = raw as object
        const existing = entries.get(key)
        if (existing !== undefined) {
          if (existing.active) existing.recursive = true
          if (existing.recursive || existing.active) {
            return {
              type: t.ref(existing.id),
              pair: {
                toValue: (value) => existing.pair!.toValue(value),
                fromValue: (value) => existing.pair!.fromValue(value),
              },
            }
          }
          return yield* buildNode(alreadyEncoded ? raw : SchemaAST.toEncoded(raw))
        }
        const entry: Entry = {
          id: `effect-schema-${nextId++}`,
          active: true,
          recursive: false,
        }
        entries.set(key, entry)
        const a = alreadyEncoded ? raw : SchemaAST.toEncoded(raw)
        entries.set(a as object, entry)
        const built = yield* buildNode(a)
        entry.active = false
        entry.pair = built.pair
        const type = { ...built.type, metadata: metadataOf(a) }
        if (entry.recursive) {
          const title = (a.annotations as { title?: unknown } | undefined)?.title
          defs.set(entry.id, {
            ...(typeof title === "string" ? { name: title } : {}),
            body: type,
          })
          return { type: t.ref(entry.id), pair: built.pair }
        }
        return { type, pair: built.pair }
      })

    const unionNode = (
      a: SchemaAST.Union,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        // String-literal enum: every member is a string Literal.
        if (
          a.types.length > 0 &&
          a.types.every(
            (m): m is SchemaAST.Literal => SchemaAST.isLiteral(m) && typeof m.literal === "string",
          )
        ) {
          const literals = a.types.map((m) => m.literal as string)
          return {
            type: t.enum(literals),
            pair: {
              toValue: (s: string) => v.enum(literals.indexOf(s)),
              fromValue: (sv) => literals[(sv as { caseIndex: number }).caseIndex]!,
            },
          }
        }

        // NullOr / UndefinedOr: a union with exactly one Null OR Undefined/Void
        // member and exactly one "real" member maps to `option<inner>`.
        // NullishOr (Null + Undefined + T) is intentionally NOT collapsed — both
        // empty kinds can't round-trip through a single option, so it falls
        // through to the generic variant path.
        const emptyMembers = a.types.filter(isNullOrUndefinedAST)
        const realMembers = a.types.filter((m) => !isNullOrUndefinedAST(m))
        if (emptyMembers.length === 1 && realMembers.length === 1) {
          const empty =
            emptyMembers[0]!._tag === "Null" ? ("null" as const) : ("undefined" as const)
          const { type, pair } = yield* child(realMembers[0]!)
          return optionWrap(type, pair, { kind: empty })
        }

        if (a.types.length === 0) {
          return yield* unsupported("empty union")
        }

        // Tagged variant: every member is an Objects with a *required*
        // string-literal _tag. Falls through to generic variant otherwise.
        if (a.types.every(SchemaAST.isObjects)) {
          const tagged = a.types.every((m) => {
            const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag")
            return (
              !!tagPs &&
              !SchemaAST.isOptional(tagPs.type) &&
              SchemaAST.isLiteral(tagPs.type) &&
              typeof tagPs.type.literal === "string"
            )
          })
          if (tagged) {
            return yield* taggedVariantNode(a as SchemaAST.Union<SchemaAST.Objects>)
          }
        }

        // Generic variant: arbitrary union members.
        return yield* genericVariantNode(a)
      })

    /**
     * Build a tagged variant where every member is an Objects with a
     * string-literal `_tag` discriminator.
     */
    const taggedVariantNode = (
      a: SchemaAST.Union<SchemaAST.Objects>,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        type Case = {
          readonly tag: string
          readonly payloadType: SchemaType | undefined
          readonly payloadPair: ValuePair | undefined
        }
        const cases: Array<Case> = []
        const seenTags = new Set<string>()
        for (const m of a.types) {
          const tagPs = m.propertySignatures.find((ps) => ps.name === "_tag")!
          const tag = (tagPs.type as SchemaAST.Literal).literal as string
          if (seenTags.has(tag)) {
            return yield* unsupported(`duplicate variant tag '${tag}' in tagged union`)
          }
          seenTags.add(tag)
          const rest = m.propertySignatures.filter((ps) => ps.name !== "_tag")
          if (rest.length === 0) {
            cases.push({ tag, payloadType: undefined, payloadPair: undefined })
          } else {
            const { type, pair } = yield* recordNode(rest)
            cases.push({ tag, payloadType: type, payloadPair: pair })
          }
        }
        const tagToIdx = new Map(cases.map((c, i) => [c.tag, i] as const))
        const variantCases: Array<VariantCaseType> = cases.map((c) =>
          variantCase(c.tag, c.payloadType, c.payloadType?.metadata),
        )
        return {
          type: t.variant(variantCases),
          pair: {
            toValue: (obj: { _tag: string } & Record<string, unknown>) => {
              const i = tagToIdx.get(obj._tag)
              if (i === undefined) throw new Error(`unknown variant tag: ${obj._tag}`)
              const c = cases[i]!
              if (c.payloadPair === undefined) return v.variant(i, undefined)
              const { _tag, ...rest } = obj
              void _tag
              return v.variant(i, c.payloadPair.toValue(rest))
            },
            fromValue: (sv) => {
              const vv = sv as { caseIndex: number; payload?: SchemaValue }
              const c = cases[vv.caseIndex]!
              if (c.payloadPair === undefined || vv.payload === undefined) {
                return { _tag: c.tag }
              }
              return { _tag: c.tag, ...c.payloadPair.fromValue(vv.payload) }
            },
          },
        }
      })

    /**
     * Build a generic variant for an arbitrary `Schema.Union(...)`. Each member
     * becomes a variant case (auto-named `caseN` or annotated via
     * {@link withVariantCaseName}). Encode picks the first member whose
     * `EncodedShape.matches(encoded)` returns true, in declaration order; decode
     * uses the case index.
     */
    const genericVariantNode = (
      a: SchemaAST.Union,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        // Conflict detection on encoded shapes.
        const shapes = a.types.map(encodedShapeOf)
        const seen = new Set<string>()
        let plainObjectCount = 0
        let plainArrayCount = 0
        let taggedObjectCount = 0
        const literalTypeKinds = new Set<string>()
        const primitiveKinds = new Set<string>()
        for (const s of shapes) {
          if (s.tag === "object") plainObjectCount++
          if (s.tag === "array") plainArrayCount++
          if (s.tag === "object-with-tag") taggedObjectCount++
          if (s.tag === "literal") literalTypeKinds.add(typeof s.literal)
          if (
            s.tag === "string" ||
            s.tag === "number" ||
            s.tag === "boolean" ||
            s.tag === "bigint"
          ) {
            primitiveKinds.add(s.tag)
          }
          const key =
            s.tag === "literal"
              ? `literal:${typeof s.literal}:${String(s.literal)}`
              : s.tag === "object-with-tag"
                ? `object-with-tag:${s.tagLiteral}`
                : s.tag
          if (s.tag !== "object" && s.tag !== "array" && s.tag !== "unknown" && seen.has(key)) {
            return yield* unsupported(`ambiguous union: two members share encoded shape '${key}'`)
          }
          seen.add(key)
        }
        if (plainObjectCount > 1) {
          return yield* unsupported(
            "ambiguous union: multiple object members without distinct `_tag` discriminators",
          )
        }
        if (plainObjectCount > 0 && taggedObjectCount > 0) {
          return yield* unsupported(
            "ambiguous union: cannot mix plain object members with `_tag`-discriminated object members",
          )
        }
        if (plainArrayCount > 1) {
          return yield* unsupported(
            "ambiguous union: multiple array members cannot be distinguished",
          )
        }
        const primitiveOfLiteralKind: Record<string, string> = {
          string: "string",
          number: "number",
          boolean: "boolean",
          bigint: "bigint",
        }
        for (const litKind of literalTypeKinds) {
          const conflict = primitiveOfLiteralKind[litKind]
          if (conflict !== undefined && primitiveKinds.has(conflict)) {
            return yield* unsupported(
              `ambiguous union: primitive '${conflict}' member overlaps a literal of the same type`,
            )
          }
        }
        if (shapes.some((s) => s.tag === "unknown") && shapes.length > 1) {
          return yield* unsupported(
            "ambiguous union: contains a member whose encoded shape cannot be classified for variant dispatch",
          )
        }

        type Case = {
          readonly name: string
          readonly type: SchemaType | undefined
          readonly pair: ValuePair | undefined
          readonly matches: (v: unknown) => boolean
          /** Encoded value to reconstruct for unit (payload-less) cases. */
          readonly emptyEncoded: unknown
        }

        const usedNames = new Set<string>()
        const cases: Array<Case> = []
        for (let i = 0; i < a.types.length; i++) {
          const m = a.types[i]!
          const annotated = variantCaseNameOf(m)
          const name = annotated ?? `case${i}`
          if (usedNames.has(name)) {
            return yield* unsupported(`duplicate variant case name '${name}' in Schema.Union`)
          }
          usedNames.add(name)
          const shape = encodedShapeOf(m)
          if (m._tag === "Null" || m._tag === "Undefined" || m._tag === "Void") {
            cases.push({
              name,
              type: undefined,
              pair: undefined,
              matches: shape.matches,
              emptyEncoded: m._tag === "Null" ? null : undefined,
            })
          } else {
            const { type, pair } = yield* child(m)
            cases.push({ name, type, pair, matches: shape.matches, emptyEncoded: undefined })
          }
        }

        const variantCases: Array<VariantCaseType> = cases.map((c) =>
          variantCase(c.name, c.type, c.type?.metadata),
        )
        return {
          type: t.variant(variantCases),
          pair: {
            toValue: (val: unknown) => {
              for (let i = 0; i < cases.length; i++) {
                const c = cases[i]!
                if (c.matches(val)) {
                  if (c.pair === undefined) return v.variant(i, undefined)
                  return v.variant(i, c.pair.toValue(val))
                }
              }
              throw new Error(`Schema.Union: no member matched value of type ${typeof val}`)
            },
            fromValue: (sv) => {
              const vv = sv as { caseIndex: number; payload?: SchemaValue }
              const c = cases[vv.caseIndex]!
              if (c.pair === undefined || vv.payload === undefined) return c.emptyEncoded
              return c.pair.fromValue(vv.payload)
            },
          },
        }
      })

    /**
     * Recognised `Schema.declareConstructor`-based types mapped to their schema
     * shape: `Schema.Option`, `Schema.Result`, `Schema.ReadonlyMap`,
     * `Schema.HashMap`.
     */
    const declarationNode = (
      a: SchemaAST.Declaration,
    ): Effect.Effect<{ type: SchemaType; pair: ValuePair }, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const tc = declarationConstructorTag(a)
        switch (tc) {
          case "effect/Option": {
            const inner = a.typeParameters[0]
            if (inner === undefined) {
              return yield* unsupported("Schema.Option without inner type parameter")
            }
            const { type, pair } = yield* child(inner)
            return optionWrap(type, pair, { kind: "effect-option" })
          }

          case "effect/Result": {
            const okAst = a.typeParameters[0]
            const errAst = a.typeParameters[1]
            if (okAst === undefined || errAst === undefined) {
              return yield* unsupported("Schema.Result without both type parameters")
            }
            const { type: okType, pair: okPair } = yield* child(okAst)
            const { type: errType, pair: errPair } = yield* child(errAst)
            const pair: ValuePair = {
              toValue: (val) => {
                const r = val as Result.Result<unknown, unknown>
                return Result.isSuccess(r)
                  ? v.ok(okPair.toValue(r.success))
                  : v.err(errPair.toValue(r.failure))
              },
              fromValue: (sv) => {
                const rv = (sv as { result: { tag: "ok" | "err"; value?: SchemaValue } }).result
                if (rv.tag === "ok") {
                  return Result.succeed(
                    rv.value === undefined ? undefined : okPair.fromValue(rv.value),
                  )
                }
                return Result.fail(rv.value === undefined ? undefined : errPair.fromValue(rv.value))
              },
            }
            return { type: t.result(okType, errType), pair }
          }

          case "ReadonlyMap":
          case "effect/HashMap": {
            const kAst = a.typeParameters[0]
            const vAst = a.typeParameters[1]
            if (kAst === undefined || vAst === undefined) {
              return yield* unsupported("Schema.ReadonlyMap/HashMap without both type parameters")
            }
            const { type: kType, pair: kPair } = yield* child(kAst)
            const { type: vType, pair: vPair } = yield* child(vAst)
            const isHashMap = tc === "effect/HashMap"
            // Represented as `list<tuple<k, v>>` so arbitrary (non-primitive)
            // key types are allowed, matching the previous model's behaviour.
            const pair: ValuePair = {
              toValue: (val) => {
                const entries: Iterable<readonly [unknown, unknown]> = isHashMap
                  ? HashMap.toEntries(val as HashMap.HashMap<unknown, unknown>)
                  : (val as ReadonlyMap<unknown, unknown>).entries()
                const items: Array<SchemaValue> = []
                for (const [k, val2] of entries) {
                  items.push(v.tuple([kPair.toValue(k), vPair.toValue(val2)]))
                }
                return v.list(items)
              },
              fromValue: (sv) => {
                const lv = sv as { elements: ReadonlyArray<SchemaValue> }
                const entries: Array<[unknown, unknown]> = lv.elements.map((it) => {
                  const tv = it as { elements: ReadonlyArray<SchemaValue> }
                  return [kPair.fromValue(tv.elements[0]!), vPair.fromValue(tv.elements[1]!)]
                })
                return isHashMap ? HashMap.fromIterable(entries) : new Map(entries)
              },
            }
            return { type: t.list(t.tuple([kType, vType])), pair }
          }

          default:
            return yield* unsupported(`unsupported declaration: ${tc ?? "unknown"}`)
        }
      })

    // Walk on the *encoded* AST so user-defined `decodeTo` chains (Schema.Option,
    // Schema.Result, custom record↔class bridges, …) surface their wire shape.
    const root = yield* nodeFor(ast)
    return { ...root, defs }
  })
}

/**
 * Build a `WitCodec<S>` for a single Effect Schema. Composes:
 *
 *     userSchema (Type ↔ Encoded)
 *       ↕  per-AST value transforms (encoded ↔ SchemaValue)
 *
 * into one `Codec<S["Type"], SchemaValue>`. Refinements / transformations
 * inside the user's schema run as part of the outer codec, so we get validation
 * and good error messages for free. The flat `schema-value-tree` wire carrier is
 * produced from `SchemaValue` at the dispatch boundary.
 *
 * @since 1.6.0
 * @category codecs
 */
export const toWitCodec = <S extends Schema.Top>(
  schema: S,
): Effect.Effect<WitCodec<S>, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const EncodedCarrier = Schema.declare((_u): _u is S["Encoded"] => true)
    const SchemaValueCarrier = Schema.declare((_u): _u is SchemaValue => true)

    // Void/undefined returns map to WIT `output-schema.unit`; the graph + codec
    // are placeholders the agent layer ignores when `isUnit` is set.
    const encodedAst = SchemaAST.toEncoded(schema.ast)
    const isUnit = isVoidLikeAST(encodedAst)

    const walked = isUnit ? undefined : yield* walk(schema.ast)
    const pair: ValuePair = walked?.pair ?? {
      toValue: () => v.record([]),
      fromValue: () => undefined,
    }
    const root: SchemaType = walked?.type ?? t.record([])
    const graph: SchemaGraph = { defs: walked?.defs ?? new Map(), root }

    const svToEncoded = SchemaValueCarrier.pipe(
      Schema.decodeTo(EncodedCarrier, {
        decode: SchemaGetter.transformOrFail((sv: SchemaValue) => {
          const converted = Effect.try({
            try: () => {
              assertValueShape(graph, root, sv)
              return sv
            },
            catch: (error) =>
              new SchemaIssue.InvalidValue(Option.some(sv), {
                message: error instanceof Error ? error.message : String(error),
              }),
          })
          return Effect.flatMap(converted, (checked) =>
            Effect.flatMap(Effect.context<any>(), (context) =>
              Effect.try({
                try: () => withConversionContext(context, () => pair.fromValue(checked)),
                catch: (error) =>
                  new SchemaIssue.InvalidValue(Option.some(sv), {
                    message: error instanceof Error ? error.message : String(error),
                  }),
              }),
            ),
          )
        }),
        encode: SchemaGetter.transformOrFail((enc: S["Encoded"]) =>
          Effect.flatMap(Effect.context<any>(), (context) =>
            Effect.try({
              try: () => withConversionContext(context, () => pair.toValue(enc)),
              catch: (error) =>
                new SchemaIssue.InvalidValue(Option.some(enc), {
                  message: error instanceof Error ? error.message : String(error),
                }),
            }),
          ),
        ),
      }),
    )

    const codec = svToEncoded.pipe(Schema.decodeTo(schema)) as Schema.Codec<
      S["Type"],
      SchemaValue,
      S["DecodingServices"],
      S["EncodingServices"]
    >

    return {
      schema,
      graph,
      isUnit,
      codec,
    }
  })

const wireSchemaError = (error: unknown): Schema.SchemaError =>
  new Schema.SchemaError(
    new SchemaIssue.InvalidValue(Option.none(), {
      message: error instanceof Error ? error.message : String(error),
    }),
  )

/** Compile an Effect Schema and expose its canonical graph and wire codecs. */
export const compile = <S extends Schema.Top>(
  schema: S,
): Effect.Effect<CompiledWitCodec<S>, UnsupportedSchemaError> =>
  Effect.map(toWitCodec(schema), (compiled) => ({
    ...compiled,
    schemaGraph: schemaGraphToWit(compiled.graph),
    encode: (value) =>
      Effect.flatMap(Schema.encodeEffect(compiled.codec)(value), (encoded) =>
        Effect.try({ try: () => schemaValueToWit(encoded), catch: wireSchemaError }),
      ),
    encodeAsync: (value) =>
      Effect.flatMap(Schema.encodeEffect(compiled.codec)(value), (encoded) =>
        Effect.tryPromise({
          try: (signal) => schemaValueToWitAsync(encoded, signal),
          catch: wireSchemaError,
        }),
      ),
    decode: (value) => decodeFromWire(compiled.codec, value),
  }))

/** Decode a complete wire value while retaining ownership until validation succeeds.
 * @since 1.6.0 @category codecs
 */
export const decodeFromWire = <A, RD, RE>(
  codec: Schema.Codec<A, SchemaValue, RD, RE>,
  value: CoreTypes.SchemaValueTree,
): Effect.Effect<A, Schema.SchemaError, RD> =>
  withCapabilityTransaction((transaction) =>
    Effect.flatMap(
      Effect.try({
        try: () => schemaValueFromWit(value, transaction),
        catch: wireSchemaError,
      }),
      Schema.decodeEffect(codec),
    ),
  )
