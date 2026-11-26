/**
 * Typed agent IDs. An agent ID is a UUID that Golem issues; its text form is the UUID.
 *
 * @since 1.6.0
 */
import type { Brand } from "effect"
import { Effect } from "effect"

/**
 * The ID of an agent within its environment: its lower-case hyphenated UUID text, branded so
 * that other strings are not accepted where an agent ID is expected. Compare IDs with `===`, use
 * them as `Map` keys, and store them as text; {@link parse} reads the text back. Use the
 * `Ids.AgentId` schema to carry an ID in method inputs, outputs, and snapshot state.
 *
 * @since 1.6.0
 * @category models
 */
export type AgentId = string & Brand.Brand<"AgentId">

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
      ? Effect.succeed(text.toLowerCase() as AgentId)
      : Effect.fail(new AgentIdParseError(text, `invalid UUID '${text}'`)),
  )
