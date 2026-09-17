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
import { Schema } from "effect"
import { Uint64 } from "./WitTypes.js"

/**
 * Schema for `golem:core/types@2.0.0`.Uuid.
 *
 * @since 1.5.1
 * @category codecs
 */
export const Uuid = Schema.Struct({
  highBits: Uint64,
  lowBits: Uint64,
})

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
