/**
 * Type-only drift detection for WIT-originated types that the SDK
 * re-shapes into a richer JS / Effect / Schema surface.
 *
 * Each entry below pins a relationship between an SDK construct and
 * its underlying WIT type — derived from real code on BOTH sides, so
 * the assertion only fires when the WIT side actually drifts away
 * from what the wrapper still claims to mirror. Hand-written copies
 * of d.ts shapes are not allowed here: that is just a host-API
 * snapshot under another name (see AGENTS.md "WIT-drift suite").
 *
 * Variant-tag exhaustiveness checks for namespace constructors
 * (`PersistenceLevel`, `FunctionType`, `RevertTarget`, `Filter`, …)
 * live ALONGSIDE the wrappers themselves as `satisfies Record<TagUnion,
 * unknown>` clauses; on regen of `golem-types/*.d.ts`, a new variant
 * trips the `satisfies` clause directly at the wrapper file. This
 * file therefore only carries shape-pins for SDK Schema codecs
 * mirroring WIT records.
 *
 * Consumed solely by `tsc --noEmit` (run via `npm run typecheck`);
 * vitest does not pick it up because the name does not end in
 * `.test.ts`.
 */

import * as Datetime from "../src/Datetime.js"
import * as Ids from "../src/Ids.js"
import * as AgentRuntime from "../src/index.js"
import * as MiddlewareRuntime from "../src/Middleware.js"
import type * as CoreTypes from "golem:core/types@2.0.0"

void (AgentRuntime satisfies typeof import("agent-guest"))
void (AgentRuntime satisfies typeof import("agent-tool-middleware-guest"))
void (MiddlewareRuntime satisfies typeof import("tool-middleware-guest"))

/**
 * Structural mutual-assignability check, recursively normalising
 * `readonly` modifiers. Schema-codec `.Type` projections add
 * `readonly` to every field while WIT records are emitted as
 * mutable, so a strict invariant `Equal` would always fail; we
 * only care that the two shapes are interchangeable at the wire.
 */
type Mutable<T> = T extends (...args: never[]) => unknown
  ? T
  : T extends ReadonlyArray<infer U>
    ? Array<Mutable<U>>
    : T extends object
      ? { -readonly [K in keyof T]: Mutable<T[K]> }
      : T

type StructEqual<A, B> = [Mutable<A>] extends [Mutable<B>]
  ? [Mutable<B>] extends [Mutable<A>]
    ? true
    : false
  : false

/**
 * Forces every value in the supplied record to be `true`. Concrete
 * mismatches surface in the type-checker error message with the
 * record's key naming the offending symbol.
 */
type AssertAllTrue<T extends Record<string, true>> = T

/**
 * Canonical Golem identifier schemas. `AccountId` and `EnvironmentId`
 * are included because the public SDK also exposes those opaque IDs;
 * scalar aliases such as `OplogIndex`, `NodeIndex`, and `ResourceId`
 * continue to use the existing `WitTypes` primitive codecs.
 */
export type _Drift_IdSchemaCodecs = AssertAllTrue<{
  "Ids.Uuid": StructEqual<typeof Ids.Uuid.Type, CoreTypes.Uuid>
  "Ids.ComponentId": StructEqual<typeof Ids.ComponentId.Type, CoreTypes.ComponentId>
  "Ids.AgentId": StructEqual<typeof Ids.AgentId.Type, CoreTypes.AgentId>
  "Ids.AccountId": StructEqual<typeof Ids.AccountId.Type, CoreTypes.AccountId>
  "Ids.EnvironmentId": StructEqual<typeof Ids.EnvironmentId.Type, CoreTypes.EnvironmentId>
  "Ids.PromiseId": StructEqual<typeof Ids.PromiseId.Type, CoreTypes.PromiseId>
}>

/**
 * Canonical datetime schema matching the WASI wall-clock record.
 */
export type _Drift_DatetimeSchemaCodec = AssertAllTrue<{
  "Datetime.Datetime": StructEqual<typeof Datetime.Datetime.Type, CoreTypes.Datetime>
}>
