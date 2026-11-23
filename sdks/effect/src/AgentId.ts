/**
 * Typed agent IDs. An agent ID is a UUID that Golem issues; its text form is the UUID.
 *
 * @since 1.6.0
 */
import { Effect } from "effect"
import type { Uuid } from "./Ids.js"
import { typedAgentId } from "./internal/agentId.js"

/**
 * The ID of an agent within its environment. The `toString` of an ID that the SDK produces
 * returns the UUID text, the same text as {@link format}. Use the `Ids.AgentId` schema to carry
 * an ID in method inputs, outputs, and snapshot state.
 *
 * @since 1.6.0
 * @category models
 */
export interface AgentId {
  readonly uuid: Uuid
}

/**
 * A text is not a valid agent ID.
 *
 * @since 1.6.0
 * @category errors
 */
export class AgentIdParseError {
  readonly _tag = "AgentIdParseError"
  constructor(
    readonly input: string,
    readonly reason: string,
  ) {}
}

const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * Format an agent ID as its UUID text.
 *
 * @since 1.6.0
 * @category conversions
 */
export const format = (id: AgentId): string => id.uuid

/**
 * Parse the UUID text form of an agent ID. Upper-case hex is accepted and normalized to lower
 * case. The parse checks only the text form: whether an agent has the ID is known only to the
 * host, for example through a client's `getById`.
 *
 * @since 1.6.0
 * @category conversions
 */
export const parse = (text: string): Effect.Effect<AgentId, AgentIdParseError> =>
  Effect.suspend(() =>
    uuidPattern.test(text)
      ? Effect.succeed(typedAgentId(text.toLowerCase() as Uuid))
      : Effect.fail(new AgentIdParseError(text, `invalid UUID '${text}'`)),
  )
