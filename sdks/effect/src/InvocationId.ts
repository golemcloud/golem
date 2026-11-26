/**
 * Typed IDs of agent invocations. The text form of an invocation ID is `<agent-id>.<uuid>`,
 * where `<agent-id>` is the UUID text of the agent ID.
 *
 * @since 1.6.0
 */
import { Effect, Schema, Scope } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import { parseUuid } from "golem:core/types@2.0.0"
import type { AgentId } from "./AgentId.js"
import { decodeMethodOutput, wireMethodOf, type RemoteCallError } from "./Client.js"
import { RpcClient } from "./host/RpcClient.js"
import type { Uuid } from "./Ids.js"
import { agentIdToWit } from "./internal/agentId.js"
import { makeInvocationId } from "./internal/invocationId.js"
import { scopedInvocation } from "./internal/rpc.js"
import type { MethodParams, MethodSpec, MethodSuccess, MethodSuccessType } from "./Method.js"
import type { UnsupportedSchemaError } from "./WitCodec.js"

/**
 * The ID of one invocation of an agent: the agent ID and the UUID of the invocation. The
 * `toString` of an ID that the SDK produces returns the `<agent-id>.<uuid>` text form, the same
 * text as {@link format}.
 *
 * @since 1.6.0
 * @category models
 */
export interface InvocationId {
  readonly agentId: AgentId
  readonly uuid: Uuid
}

/**
 * A text is not a valid invocation ID.
 *
 * @since 1.6.0
 * @category errors
 */
export class InvocationIdParseError {
  readonly _tag = "InvocationIdParseError"
  constructor(
    readonly input: string,
    readonly reason: string,
  ) {}
}

const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * Format an invocation ID as `<agent-id>.<uuid>`.
 *
 * @since 1.6.0
 * @category conversions
 */
export const format = (id: InvocationId): string => `${id.agentId}.${id.uuid}`

/**
 * Parse the `<agent-id>.<uuid>` text form. Both parts must be UUIDs in the hyphenated form;
 * upper-case hex is accepted and normalized to lower case.
 *
 * @since 1.6.0
 * @category conversions
 */
export const parse = (text: string): Effect.Effect<InvocationId, InvocationIdParseError> =>
  Effect.suspend(() => {
    const fail = (reason: string) => Effect.fail(new InvocationIdParseError(text, reason))
    const parts = text.split(".")
    if (parts.length !== 2) return fail("expected `<agent-id>.<uuid>`")
    const [agentId, uuid] = parts as [string, string]
    if (!uuidPattern.test(agentId)) return fail(`invalid agent ID '${agentId}'`)
    if (!uuidPattern.test(uuid)) return fail(`invalid UUID '${uuid}'`)
    return Effect.succeed(
      makeInvocationId(agentId.toLowerCase() as AgentId, uuid.toLowerCase() as Uuid),
    )
  })

/**
 * The lifecycle state of an accepted invocation: `pending`, `scheduled`, `running`, `waiting`,
 * `retrying`, `interrupted`, `completed`, `failed`, `cancelled` or `background-failed`.
 *
 * @since 1.6.0
 * @category models
 */
export type InvocationStatus = AgentHost.InvocationStatus

/**
 * The handle of an accepted invocation of an agent method, which `trigger` and `schedule`
 * return. `A` is the result of the method and `E` its typed error.
 *
 * The ID only locates the invocation; it is not authority. Only the caller that started the
 * invocation, or a caller with the applicable management permission, can observe or cancel it;
 * any other caller fails with an `RpcCallError` whose cause is `denied`. Store `id` to await the
 * invocation from a later invocation of the same agent, and rebuild the handle with {@link get}.
 * Closing the scope that owns the handle, or interrupting `awaitResult`, only detaches: the
 * invocation keeps running.
 *
 * `trigger` and `schedule` reject methods with streams, and never send a scope card. Cancelling
 * a running invocation is not supported: `cancel` fails with `RpcCannotCancelRunning`.
 *
 * @since 1.6.0
 * @category models
 */
export interface Invocation<A, E = never> {
  readonly id: InvocationId
  /**
   * The current lifecycle state. Fails with an `RpcCallError` whose cause is `not-found` when
   * the agent or the invocation no longer exists.
   */
  readonly status: Effect.Effect<InvocationStatus, RemoteCallError>
  /**
   * Awaits the result without starting, resuming or resubmitting the invocation. Fails with
   * `RpcCancelled` when the invocation was cancelled before it started. Interruption only
   * detaches.
   */
  readonly awaitResult: Effect.Effect<A, RemoteCallError | E>
  /**
   * Withdraws the invocation if it has not started yet, and succeeds with `true`. Succeeds with
   * `false` when it already finished or was already cancelled, and fails with
   * `RpcCannotCancelRunning` when it is running.
   */
  readonly cancel: Effect.Effect<boolean, RemoteCallError>
}

/**
 * Rebuild the handle of the invocation with the given ID, without a remote call. `method` is the
 * spec of the invoked method, whose success and error schemas decode the result. The current
 * scope owns the handle.
 *
 * @since 1.6.0
 * @category constructors
 */
export const get = <P extends MethodParams, S extends MethodSuccess, E extends Schema.Top>(
  id: InvocationId,
  method: MethodSpec<P, S, E>,
): Effect.Effect<
  Invocation<MethodSuccessType<S>, E["Type"]>,
  RemoteCallError | UnsupportedSchemaError,
  RpcClient | Scope.Scope
> =>
  Effect.gen(function* () {
    const wire = yield* wireMethodOf(`Invocation ${format(id)}`, method)
    const raw = yield* Effect.try({
      try: () => ({
        owner: { tag: "agent" as const, val: agentIdToWit(id.agentId) },
        id: parseUuid(id.uuid),
      }),
      catch: (cause): RemoteCallError => ({
        _tag: "InvalidUuidError",
        value: format(id),
        reason: String(cause),
      }),
    })
    const host = yield* RpcClient
    return yield* scopedInvocation(host.getInvocation(raw), (tree) =>
      decodeMethodOutput(wire, tree),
    ) as Effect.Effect<Invocation<MethodSuccessType<S>, E["Type"]>, RemoteCallError, Scope.Scope>
  })
