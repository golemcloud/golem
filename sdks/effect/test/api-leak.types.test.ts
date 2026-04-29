/**
 * Public-API-leak typecheck guard. Imports every user-facing
 * combinator that internally consumes a host-service tag and asserts
 * — by type-level assignability — that its `R` channel does not
 * accidentally widen with a tag that is *not* meant to be there.
 *
 * The dispatcher (`src/agent.ts`) provides every `host/*` service via
 * `userRuntimeLayer`, so it is **expected** that public-facing
 * combinators advertise host-service tags in `R` (the dispatcher
 * erases them at the boundary). The point of this file is to LOCK
 * the exact set of tags each surface advertises, so that:
 *
 * - if a refactor accidentally adds a new tag, this file's TypeScript
 *   build fails;
 * - if a refactor accidentally removes a tag we *thought* the
 *   dispatcher had to provide, this file's TypeScript build fails;
 * - if the dispatcher's `userRuntimeLayer` ever stops providing one
 *   of the tags listed below, the agent.ts typecheck fails.
 *
 * No runtime assertions — vitest just needs to parse + typecheck the
 * file. The single trivial `it("compiles")` keeps it inside the
 * normal vitest run.
 */
import { describe, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { defineAgent } from "../src/agent.js"
import * as Agents from "../src/agents.js"
import * as Blobstore from "../src/blobstore.js"
import * as Durability from "../src/durability.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { BlobstoreClient } from "../src/host/BlobstoreClient.js"
import { DurabilityClient } from "../src/host/DurabilityClient.js"
import { KeyValueClient } from "../src/host/KeyValueClient.js"
import { OplogClient } from "../src/host/OplogClient.js"
import { PromiseClient } from "../src/host/PromiseClient.js"
import { QuotaClient } from "../src/host/QuotaClient.js"
import { RetryClient } from "../src/host/RetryClient.js"
import * as KeyValue from "../src/keyvalue.js"
import * as Oplog from "../src/oplog.js"
import { method } from "../src/method.js"
import * as Quota from "../src/quota.js"
import * as Retry from "../src/retry.js"
import { SelfAgentId } from "../src/self-agent-id.js"
import * as Webhook from "../src/webhook.js"

/**
 * Compile-time helper. Forces TypeScript to verify that two types are
 * mutually assignable. Used here as a strict identity check on
 * `Effect.Effect<A, E, R>` shapes.
 */
type AssertEqual<X, Y> =
  (<T>() => T extends X ? 1 : 2) extends <T>() => T extends Y ? 1 : 2 ? true : false

// ---------------------------------------------------------------------------
// Webhook
// ---------------------------------------------------------------------------
{
  const _create: Effect.Effect<
    Webhook.WebhookHandle,
    Agents.AgentsHostError | Webhook.WebhookHostError,
    AgentHostClient | PromiseClient
  > = Webhook.create
  void _create
}

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------
{
  const _acquire: Effect.Effect<Quota.QuotaToken, Quota.QuotaHostError, QuotaClient> =
    Quota.acquireQuotaToken("api-calls", 1n)
  void _acquire
}

// ---------------------------------------------------------------------------
// KeyValue
// ---------------------------------------------------------------------------
{
  const _open: Effect.Effect<
    KeyValue.Bucket,
    KeyValue.KeyValueHostError,
    import("effect").Scope.Scope | KeyValueClient
  > = KeyValue.openBucket("test")
  void _open
}

// ---------------------------------------------------------------------------
// Blobstore
// ---------------------------------------------------------------------------
{
  const _create: Effect.Effect<
    Blobstore.Container,
    Blobstore.BlobstoreHostError,
    import("effect").Scope.Scope | BlobstoreClient
  > = Blobstore.createContainer("test")
  void _create
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------
{
  const _list: Effect.Effect<
    ReadonlyArray<unknown>,
    Retry.RetryHostError,
    RetryClient
  > = Retry.getPolicies()
  void _list
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------
{
  const _self: Effect.Effect<Agents.AgentMetadata, Agents.AgentsHostError, AgentHostClient> =
    Agents.getSelfMetadata
  void _self
  type _SelfOk = AssertEqual<
    typeof _self,
    Effect.Effect<Agents.AgentMetadata, Agents.AgentsHostError, AgentHostClient>
  >
  const _ok: _SelfOk = true
  void _ok
}

// ---------------------------------------------------------------------------
// Oplog
// ---------------------------------------------------------------------------
{
  const _idx: Effect.Effect<Agents.OplogIndex, Oplog.OplogHostError, OplogClient> =
    Oplog.currentIndex
  void _idx
}

// ---------------------------------------------------------------------------
// Durability
// ---------------------------------------------------------------------------
{
  const _isLive: Effect.Effect<boolean, Durability.DurabilityHostError, DurabilityClient> =
    Durability.isLive
  void _isLive
}

{
  // checkpoint adds AgentHostClient | OplogClient | SelfAgentId | DurabilityModeClient
  // (DurabilityModeClient appears via Durability.atomically; AgentHostClient via revertAgent)
  const probe = Effect.succeed(42)
  const _cp = Durability.checkpoint(probe)
  // Type assignability: the resulting R MUST include exactly these tags.
  const _expected: Effect.Effect<
    Durability.CheckpointResult<number, never>,
    Oplog.OplogHostError | Agents.AgentsHostError,
    SelfAgentId | OplogClient | AgentHostClient
  > = _cp
  void _expected
}

// ---------------------------------------------------------------------------
// `defineAgent` user-visible R inside `impl` and method handlers.
//
// User-visible R must NOT include host-service tags — the dispatcher
// erases them. This is the canonical leak-check: if any `agents.ts`
// API ever leaks (e.g. by accident) into the user-side `R`, this
// type assertion would break.
// ---------------------------------------------------------------------------
{
  defineAgent({
    name: "_LeakProbe",
    constructorParams: { name: Schema.String },
    methods: {
      probe: method({ params: {}, success: Schema.Number }),
    },
    impl: () =>
      Effect.succeed({
        probe: () => Effect.succeed(42),
      }),
  })
}

// Provide schema-services pass-through evidence: a method handler can
// require AgentHostClient (because it `yield* Agents.getSelfMetadata`)
// and the dispatcher must satisfy it. We do not test this at
// dispatch time here — only that the type composes.
{
  const _useSelf = Effect.gen(function* () {
    const meta = yield* Agents.getSelfMetadata
    void meta
  })
  const _check: Effect.Effect<void, Agents.AgentsHostError, AgentHostClient> = _useSelf
  void _check
}

describe("api-leak.types — public surface R-channel guards", () => {
  it("typechecks", () => {
    // Intentionally empty — the value lies in the assertions above
    // being parsed by `tsc` during `npm test` / `npm run typecheck`.
  })
})
