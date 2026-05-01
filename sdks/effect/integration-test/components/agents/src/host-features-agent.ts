/**
 * HostFeatures — exercises the `effect-golem` Effect-typed wrappers
 * around the Golem host APIs (`Durability`, `Oplog`, `Agents`,
 * `SelfAgentId`) inside a real Golem runtime.
 *
 * Each method is a small, focused probe of one wrapper:
 *
 * - `oplogIndex()` — `Oplog.currentIndex` returns the current oplog
 *   index as a string (so it round-trips cleanly through JSON).
 * - `withAtomic({ by })` — runs an increment inside
 *   `Durability.atomically` so the oplog gets surrounding
 *   `begin-atomic-region` / `end-atomic-region` markers.
 * - `withPersistNothing({ by })` — runs an increment inside
 *   `Durability.withPersistenceLevel(persistNothing, ...)`. Verifies
 *   that the wrapper restores the previous level on exit.
 * - `idempotencyKey()` — `Durability.generateIdempotencyKey` returned
 *   as a canonical UUID string.
 * - `selfMetadata()` — selected fields from `Agents.getSelfMetadata`,
 *   exposed as plain JSON-friendly values.
 * - `forkSelf()` — calls `Agents.fork` and returns whether we are the
 *   `original` or `forked` side.
 * - `readOplog({ count })` — reads the first N entries of the agent's
 *   own oplog via `Oplog.read` and returns their `tag` strings.
 * - `searchOplog({ query, count })` — runs `Oplog.search` against the
 *   agent's own oplog and returns the matched entry tags.
 * - `promiseRoundtrip({ payload })` — creates a host promise, completes
 *   it with the given payload, awaits it, and returns the round-tripped
 *   string. Exercises the full `Agents.Promises` API surface.
 * - `wrappedQuote({ symbol })` — wraps a non-deterministic
 *   `Math.random()` "price" in `Durability.wrap`. On the live first
 *   call the persisted oplog entry encodes `Result.succeed({price})`
 *   under function-name `host-features::wrappedQuote` and
 *   function-type `write-remote`. On replay (e.g. after `agent
 *   update --await ... manual`) the same `price` must come back
 *   without re-running the random body — exactly the durability
 *   round-trip.
 * - `wrappedQuoteFailing({ symbol })` — same shape but the body
 *   returns a typed `Effect.fail`; verifies typed-failure
 *   round-tripping through the oplog.
 *
 * Snapshotting is enabled with a small Ref-backed counter so the
 * harness can also verify the auto-snapshot path lights up after 10
 * invocations.
 */
import { Effect, Ref, Schema, Stream } from "effect"
import { Agents, defineAgent, Durability, method, Oplog, SelfAgentId, Snapshot } from "effect-golem"
import * as CoreTypes from "golem:core/types@1.5.0"

const uuidToString = (uuid: { highBits: bigint; lowBits: bigint }): string =>
  CoreTypes.uuidToString(uuid)

export const HostFeatures = defineAgent({
  name: "HostFeatures",
  description:
    "Probe agent that exercises Durability/Oplog/Agents wrappers against a real Golem runtime",
  mode: "durable",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    oplogIndex: method({ params: {}, success: Schema.String }),
    withAtomic: method({ params: { by: Schema.Number }, success: Schema.Number }),
    withPersistNothing: method({ params: { by: Schema.Number }, success: Schema.Number }),
    idempotencyKey: method({ params: {}, success: Schema.String }),
    selfMetadata: method({
      params: {},
      success: Schema.Struct({
        agentName: Schema.String,
        componentRevision: Schema.String,
        status: Schema.String,
        retryCount: Schema.String,
      }),
    }),
    forkSelf: method({
      params: {},
      success: Schema.Literals(["original", "forked"]),
    }),
    readOplog: method({
      params: { count: Schema.Number },
      success: Schema.Array(Schema.String),
    }),
    searchOplog: method({
      params: { query: Schema.String, count: Schema.Number },
      success: Schema.Array(Schema.String),
    }),
    promiseRoundtrip: method({
      params: { payload: Schema.String },
      success: Schema.String,
    }),
    wrappedQuote: method({
      params: { symbol: Schema.String },
      success: Schema.Struct({ symbol: Schema.String, price: Schema.Number }),
    }),
    wrappedQuoteFailing: method({
      params: { symbol: Schema.String },
      success: Schema.Struct({ symbol: Schema.String, price: Schema.Number }),
      error: Schema.Struct({ code: Schema.String, symbol: Schema.String }),
    }),
  },
  impl: (_input, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ count: 0 })

      return {
        oplogIndex: () =>
          Effect.gen(function* () {
            const idx = yield* Oplog.currentIndex
            return idx.toString()
          }),

        withAtomic: ({ by }) =>
          Durability.atomically(
            Ref.updateAndGet(state, (s) => ({ count: s.count + by })).pipe(
              Effect.map((s) => s.count),
            ),
          ),

        withPersistNothing: ({ by }) =>
          Durability.withPersistenceLevel(
            Durability.PersistenceLevel.persistNothing,
            Ref.updateAndGet(state, (s) => ({ count: s.count + by })).pipe(
              Effect.map((s) => s.count),
            ),
          ),

        idempotencyKey: () =>
          Effect.gen(function* () {
            const uuid = yield* Durability.generateIdempotencyKey
            return uuidToString(uuid)
          }),

        selfMetadata: () =>
          Effect.gen(function* () {
            const meta = yield* Agents.getSelfMetadata
            return {
              agentName: meta.agentId.agentId,
              componentRevision: meta.componentRevision.toString(),
              status: meta.status,
              retryCount: meta.retryCount.toString(),
            }
          }),

        forkSelf: () =>
          Effect.gen(function* () {
            const result = yield* Agents.fork
            return result.tag
          }),

        readOplog: ({ count }) =>
          Effect.gen(function* () {
            const self = yield* SelfAgentId.SelfAgentId
            const tags = yield* Stream.runCollect(
              Oplog.read({ agentId: self, start: 0n }).pipe(
                Stream.map((entry) => entry.tag),
                Stream.take(count),
              ),
            )
            return Array.from(tags)
          }),

        searchOplog: ({ query, count }) =>
          Effect.gen(function* () {
            const self = yield* SelfAgentId.SelfAgentId
            const tags = yield* Stream.runCollect(
              Oplog.search({ agentId: self, text: query }).pipe(
                Stream.map(([, entry]) => entry.tag),
                Stream.take(count),
              ),
            )
            return Array.from(tags)
          }),

        promiseRoundtrip: ({ payload }) =>
          Effect.gen(function* () {
            const id = yield* Agents.Promises.create
            const bytes = new TextEncoder().encode(payload)
            yield* Agents.Promises.complete(id, bytes)
            const out = yield* Agents.Promises.await(id)
            return new TextDecoder().decode(out)
          }),

        wrappedQuote: ({ symbol }) =>
          // The body returns Math.random — a non-deterministic value
          // that would diverge across replays without `Durability.wrap`.
          // The first call persists `Result.succeed({symbol, price})`
          // to the oplog; subsequent replays return that exact value
          // without re-rolling the dice.
          Durability.wrap(
            {
              iface: "host-features",
              function: "wrappedQuote",
              functionType: Durability.FunctionType.writeRemote,
              requestSchema: Schema.Struct({ symbol: Schema.String }),
              success: Schema.Struct({
                symbol: Schema.String,
                price: Schema.Number,
              }),
            },
            { symbol },
            Effect.sync(() => ({
              symbol,
              price: Math.round(Math.random() * 1_000_000) / 100,
            })),
          ),

        wrappedQuoteFailing: ({ symbol }) =>
          Durability.wrap(
            {
              iface: "host-features",
              function: "wrappedQuoteFailing",
              functionType: Durability.FunctionType.writeRemote,
              requestSchema: Schema.Struct({ symbol: Schema.String }),
              success: Schema.Struct({
                symbol: Schema.String,
                price: Schema.Number,
              }),
              error: Schema.Struct({
                code: Schema.String,
                symbol: Schema.String,
              }),
            },
            { symbol },
            Effect.fail({ code: "UNAVAILABLE", symbol }),
          ),
      }
    }),
})
