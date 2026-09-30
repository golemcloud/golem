/**
 * Canonical Effect Schema codecs for Golem's structured identifiers.
 *
 * These schemas mirror the existing `golem:core/types@2.0.0` and
 * `golem:api/host@1.5.0` WIT records. They can be reused as agent method
 * parameters, method results, constructor parameters, and snapshot fields
 * without copying the nested record shapes into application code.
 *
 * @since 1.5.1
 */
import { Schema, SchemaGetter } from "effect"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import { Uint64, witSchemaNodeAnnotationKey } from "./WitTypes.js"

/**
 * Encoded schema for `golem:core/types@2.0.0`.Uuid.
 *
 * @since 1.5.1
 * @category codecs
 */
const EncodedUuid = Schema.Struct({
  highBits: Uint64,
  lowBits: Uint64,
}).annotate({ [witSchemaNodeAnnotationKey]: { tag: "uuid" } })

const CanonicalUuidString = Schema.String.check(
  Schema.isGUID(),
  Schema.isPattern(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/),
).pipe(Schema.brand("GolemUuid"))

/**
 * Schema for a Golem UUID, represented in Effect programs as a branded GUID string.
 *
 * @since 1.5.1
 * @category codecs
 */
export const Uuid = EncodedUuid.pipe(
  Schema.decodeTo(CanonicalUuidString, {
    decode: SchemaGetter.transform(uuidToString),
    encode: SchemaGetter.transform(parseUuid),
  }),
)

/**
 * A Golem UUID in canonical string form.
 *
 * @since 1.5.1
 * @category models
 */
export type Uuid = typeof Uuid.Type

/**
 * Schema for `golem:core/types@2.0.0`.ComponentId.
 *
 * @since 1.5.1
 * @category codecs
 */
export const ComponentId = Schema.Struct({
  uuid: Uuid,
})

/**
 * Schema for `golem:core/types@2.0.0`.AgentId.
 *
 * @since 1.5.1
 * @category codecs
 */
export const AgentId = Schema.Struct({
  componentId: ComponentId,
  agentId: Schema.String,
})

/**
 * Schema for `golem:core/types@2.0.0`.AccountId.
 *
 * @since 1.5.1
 * @category codecs
 */
export const AccountId = Schema.Struct({
  uuid: Uuid,
})

/**
 * Schema for `golem:api/host@1.5.0`.EnvironmentId.
 *
 * @since 1.5.1
 * @category codecs
 */
export const EnvironmentId = Schema.Struct({
  uuid: Uuid,
})

/**
 * Schema for `golem:core/types@2.0.0`.PromiseId.
 *
 * @since 1.5.1
 * @category codecs
 */
export const PromiseId = Schema.Struct({
  agentId: AgentId,
  oplogIdx: Uint64,
})
