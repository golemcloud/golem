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
import { Effect, Option, Schema } from "effect"
import { defineAgent } from "../src/Agent.js"
import * as Agents from "../src/Agents.js"
import * as Blobstore from "../src/Blobstore.js"
import * as Bridge from "../src/Bridge.js"
import { AgentHostClient } from "../src/host/AgentHostClient.js"
import { BlobstoreClient } from "../src/host/BlobstoreClient.js"
import { KeyValueClient } from "../src/host/KeyValueClient.js"
import { PermissionsClient } from "../src/host/PermissionsClient.js"
import { PromiseClient } from "../src/host/PromiseClient.js"
import { QuotaClient } from "../src/host/QuotaClient.js"
import { RetryClient } from "../src/host/RetryClient.js"
import { RpcClient } from "../src/host/RpcClient.js"
import * as KeyValue from "../src/KeyValue.js"
import * as Permissions from "../src/Permissions.js"
import * as WitTypes from "../src/WitTypes.js"
import * as Capability from "../src/Capability.js"
import { method } from "../src/Method.js"
import * as Quota from "../src/Quota.js"
import * as Retry from "../src/Retry.js"
import * as Webhook from "../src/Webhook.js"

const ClientProbe = defineAgent({
  name: "_ClientProbe",
  id: {},
  methods: { ping: method({ input: {}, success: Schema.Void }) },
})

{
  const _get: Effect.Effect<
    ReturnType<typeof ClientProbe.client.get> extends Effect.Effect<infer A, any, any> ? A : never,
    | import("../src/Client.js").RemoteCallError
    | import("../src/WitCodec.js").UnsupportedSchemaError
    | import("../src/Config.js").ConfigError,
    RpcClient | import("effect").Scope.Scope
  > = ClientProbe.client.get({})
  void _get

  // @ts-expect-error raw config values bypass declaration validation and are not public input
  ClientProbe.client.get({}, { agentConfig: [] })
}

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
// Permissions
// ---------------------------------------------------------------------------
{
  type Failure = Permissions.PermissionError | Permissions.PermissionsHostError
  const card = undefined as unknown as Permissions.PermissionCard
  const template = undefined as unknown as Permissions.PolymorphicPermissionCard
  const scope = undefined as unknown as Permissions.ScopeCard
  const bounds: Permissions.Bounds = {}
  const _id: Effect.Effect<
    Permissions.CardId,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.id(card)
  const _parents: Effect.Effect<
    ReadonlyArray<Permissions.CardId>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.parents(card)
  const _inspect: Effect.Effect<Permissions.CardView, Failure, PermissionsClient> =
    Permissions.inspect(card)
  const _derive: Effect.Effect<Permissions.PermissionCard, Failure, PermissionsClient> =
    Permissions.derive(card, bounds, Permissions.Lifespan.fromIso8601("P1D"))
  const _deriveFromWallet: Effect.Effect<Permissions.PermissionCard, Failure, PermissionsClient> =
    Permissions.deriveFromWallet(bounds)
  const _deriveScope: Effect.Effect<Permissions.ScopeCard, Failure, PermissionsClient> =
    Permissions.deriveScope([card, scope], bounds)
  const _scopeId: Effect.Effect<
    Permissions.CardId,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.id(scope)
  const _scopeParents: Effect.Effect<
    ReadonlyArray<Permissions.CardId>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.parents(scope)
  const _scopeInspect: Effect.Effect<Permissions.CardView, Failure, PermissionsClient> =
    Permissions.inspect(scope)
  const _expiresAt: Effect.Effect<
    Option.Option<Permissions.Timestamp>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.expiresAt(card)
  const _scopeExpiresAt: Effect.Effect<
    Option.Option<Permissions.Timestamp>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.expiresAt(scope)
  const _release: Effect.Effect<void, Permissions.PermissionsHostError, PermissionsClient> =
    Permissions.release(card)
  const _scopeRelease: Effect.Effect<void, Permissions.PermissionsHostError, PermissionsClient> =
    Permissions.release(scope)
  const _walletVersion: Effect.Effect<
    Permissions.WalletVersionToken,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.walletVersion
  const _listModules: Effect.Effect<
    ReadonlyArray<Permissions.KernelModule>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.listModules
  const _revoke: Effect.Effect<number, Failure, PermissionsClient> = Permissions.revoke(card)
  const _install: Effect.Effect<void, Failure, PermissionsClient> = Permissions.install(
    card,
    Permissions.Holder.app("00000000-0000-0000-0000-000000000000"),
  )
  const _wallet: Effect.Effect<
    ReadonlyArray<Permissions.WalletCard>,
    Permissions.PermissionsHostError,
    PermissionsClient
  > = Permissions.wallet
  const _walletScoped: Effect.Effect<
    ReadonlyArray<Permissions.WalletCard>,
    Permissions.PermissionsHostError,
    PermissionsClient | import("effect").Scope.Scope
  > = Permissions.walletScoped
  const _holder: Effect.Effect<
    Permissions.Holder,
    Permissions.PermissionsHostError,
    AgentHostClient
  > = Permissions.agentHolder(undefined as never)
  const _validate: Effect.Effect<void, Failure, PermissionsClient> = Permissions.validateGrant({
    class: "kv",
    owner: "?env",
    recipient: "*",
    verb: "read",
    resourceId: "*",
  })
  const _sameCard: AssertEqual<Permissions.PermissionCard, Capability.PermissionCard> = true
  const _sameTemplate: AssertEqual<
    Permissions.PolymorphicPermissionCard,
    Capability.PolymorphicPermissionCard
  > = true
  const _bridgeTemplate: AssertEqual<
    Permissions.PolymorphicPermissionCard,
    Bridge.PolymorphicPermissionCard
  > = true
  const _templateDerive: Effect.Effect<Permissions.PermissionCard, Failure, PermissionsClient> =
    Permissions.derive(template, bounds)
  const _templateOps = [
    Permissions.id(template),
    Permissions.parents(template),
    Permissions.expiresAt(template),
    Permissions.inspect(template),
    Permissions.revoke(template),
    Permissions.install(template, Permissions.Holder.app("00000000-0000-0000-0000-000000000000")),
    Permissions.release(template),
    Permissions.deriveScope([template, card, scope], bounds),
  ]
  const _walletEntry = (entry: Permissions.WalletCard) => {
    switch (entry._tag) {
      case "Concrete": {
        const _concrete: Permissions.PermissionCard = entry.card
        return _concrete
      }
      case "Template": {
        const _polymorphic: Permissions.PolymorphicPermissionCard = entry.card
        return _polymorphic
      }
      case "Scope": {
        const _scope: Permissions.ScopeCard = entry.card
        return _scope
      }
    }
  }
  const _templateSchema: Schema.Codec<Permissions.PolymorphicPermissionCard> =
    WitTypes.PolymorphicPermissionCard
  const _cardSchema: Schema.Codec<Permissions.PermissionCard> = WitTypes.PermissionCard
  void [_sameTemplate, _bridgeTemplate, _templateDerive, _templateOps, _walletEntry]
  void [_templateSchema, _cardSchema]
  const _bridgeCard: AssertEqual<Permissions.PermissionCard, Bridge.PermissionCard> = true
  const _bridgeScope: AssertEqual<Permissions.ScopeCard, Bridge.ScopeCard> = true
  void [
    _scopeId,
    _scopeParents,
    _scopeInspect,
    _expiresAt,
    _scopeExpiresAt,
    _release,
    _scopeRelease,
    _walletVersion,
    _listModules,
    _bridgeCard,
    _bridgeScope,
    _id,
    _parents,
    _inspect,
    _derive,
    _deriveFromWallet,
    _deriveScope,
    _revoke,
    _install,
    _wallet,
    _walletScoped,
    _holder,
    _validate,
    _sameCard,
  ]

  const _negative = () => {
    // @ts-expect-error card IDs have no public constructor
    void new Permissions.CardId()
    // @ts-expect-error lifespans are built through fromIso8601 or fromDuration
    void new Permissions.Lifespan("P1D")
    // @ts-expect-error the public card type is type-only and has no runtime constructor
    void Capability.PermissionCard
    // @ts-expect-error plain data is not a card
    void Permissions.revoke({ lowerPositive: [] })
    // @ts-expect-error scope cards cannot be revoked
    void Permissions.revoke(scope)
    // @ts-expect-error scope cards cannot be installed
    void Permissions.install(scope, Permissions.Holder.app("00000000-0000-0000-0000-000000000000"))
    // @ts-expect-error persistent cards cannot be derived from a scope card
    void Permissions.derive(scope, bounds)
    // @ts-expect-error a scope card is not a schema permission card
    void Bridge.permissionCardToSchemaValue(scope)
    // @ts-expect-error a persistent card is not a scope card
    const _persistentAsScope: Permissions.ScopeCard = card
    void _persistentAsScope
    // @ts-expect-error a scope card is not a persistent card
    const _scopeAsPersistent: Permissions.PermissionCard = scope
    void _scopeAsPersistent
    // @ts-expect-error a template card is not a concrete card
    const _templateAsConcrete: Permissions.PermissionCard = template
    void _templateAsConcrete
    // @ts-expect-error a concrete card is not a template card
    const _concreteAsTemplate: Permissions.PolymorphicPermissionCard = card
    void _concreteAsTemplate
    // @ts-expect-error a scope card is not a template card
    const _scopeAsTemplate: Permissions.PolymorphicPermissionCard = scope
    void _scopeAsTemplate
    // @ts-expect-error a template card is not a scope card
    const _templateAsScope: Permissions.ScopeCard = template
    void _templateAsScope
    // @ts-expect-error a template card is not a concrete schema card
    void Bridge.permissionCardToSchemaValue(template)
    // @ts-expect-error a concrete card is not a polymorphic schema card
    void Bridge.polymorphicPermissionCardToSchemaValue(card)
    // @ts-expect-error a scope card is not a polymorphic schema card
    void Bridge.polymorphicPermissionCardToSchemaValue(scope)
    // @ts-expect-error a concrete card is not the card of a Template wallet entry
    const _concreteAsTemplateEntry: Permissions.WalletCard = { _tag: "Template", card }
    void _concreteAsTemplateEntry
    // @ts-expect-error a template card is not the card of a Concrete wallet entry
    const _templateAsConcreteEntry: Permissions.WalletCard = { _tag: "Concrete", card: template }
    void _templateAsConcreteEntry
    // @ts-expect-error a template card is not the card of a Scope wallet entry
    const _templateAsScopeEntry: Permissions.WalletCard = { _tag: "Scope", card: template }
    void _templateAsScopeEntry
    // @ts-expect-error an object literal is not a template card
    const _literalTemplate: Permissions.PolymorphicPermissionCard = {
      toJSON: () => undefined as never,
    }
    void _literalTemplate
    // @ts-expect-error a spread copy of a card is not a template card
    const _spreadTemplate: Permissions.PolymorphicPermissionCard = { ...template }
    void _spreadTemplate
    // @ts-expect-error a spread copy of a concrete card is not a template card
    const _spreadConcrete: Permissions.PolymorphicPermissionCard = { ...card }
    void _spreadConcrete
    // @ts-expect-error the polymorphic card type is type-only and has no runtime constructor
    void Capability.PolymorphicPermissionCard
    // @ts-expect-error a concrete schema does not carry template cards
    const _templateInConcreteSchema: Schema.Codec<Permissions.PolymorphicPermissionCard> =
      WitTypes.PermissionCard
    void _templateInConcreteSchema
    // @ts-expect-error a template card cannot be a client's default scope card
    void ClientProbe.client.get({}, { scopeCard: template })
  }
  void _negative

  const _remote = Effect.gen(function* () {
    const client = yield* ClientProbe.client.get({}, { scopeCard: scope })
    // @ts-expect-error a persistent card cannot be a client's default scope card
    yield* ClientProbe.client.get({}, { scopeCard: card })
    // @ts-expect-error a persistent card cannot be a per-call scope card
    void client.ping.withScopeCard(card)
    void client.ping.withScopeCard(scope)
    const _call: AssertEqual<
      ReturnType<ReturnType<typeof client.ping.withScopeCard>>,
      ReturnType<typeof client.ping>
    > = true
    // @ts-expect-error scope cards attach only to awaited calls
    void client.ping.trigger.withScopeCard
    void _call
  })
  void _remote
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
    id: { name: Schema.String },
    methods: {
      probe: method({ input: {}, success: Schema.Number }),
    },
  }).implement({
    init: () => Effect.void,
    methods: () =>
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
