import { Context } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"

/**
 * Identity of an authenticated agent caller. Mirrors the WIT
 * `golem:agent/common@1.5.0` `principal` discriminated union.
 *
 * Re-exported here so users of `effect-golem` do not need to import the
 * ambient `golem:agent/common@1.5.0` module directly. (Named
 * `PrincipalValue` to avoid a clash with the {@link Principal} service
 * class below — `yield* Principal` returns a `PrincipalValue`.)
 */
export type PrincipalValue = AgentCommon.Principal
export type OidcPrincipal = AgentCommon.OidcPrincipal
export type AgentPrincipal = AgentCommon.AgentPrincipal
export type GolemUserPrincipal = AgentCommon.GolemUserPrincipal

/**
 * Effect service exposing the active {@link Principal}.
 *
 * The framework provides this service automatically at the dispatch
 * boundary:
 *
 * - inside an agent's `impl` (constructor) effect it resolves to the
 *   principal the host passed to `agent-guest.guest.initialize`;
 * - inside a method handler it resolves to the principal the host
 *   passed to `agent-guest.guest.invoke` for that specific call (which
 *   may differ from the initialize-time principal, e.g. when other
 *   callers reach a durable agent instance).
 *
 * Usage:
 *
 * ```ts
 * impl: () =>
 *   Effect.gen(function* () {
 *     const owner = yield* Principal
 *     return {
 *       whoCalled: () =>
 *         Effect.gen(function* () {
 *           const caller = yield* Principal
 *           return caller.tag
 *         }),
 *     }
 *   })
 * ```
 *
 * The dispatcher always provides this service before running user code,
 * so depending on it from inside an agent never leaks into the public
 * `R` slot of method/constructor signatures. In tests that bypass the
 * dispatcher you must provide it explicitly with
 * `Effect.provideService(Principal, …)`.
 */
export class Principal extends Context.Service<Principal, AgentCommon.Principal>()(
  "effect-golem/Principal",
) {}
