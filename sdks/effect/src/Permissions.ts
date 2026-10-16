/**
 * Effect-typed permission-card management over `golem:permissions@0.1.0`.
 *
 * A {@link PermissionCard} is the opaque affine capability that agent schemas already carry
 * (`WitTypes.PermissionCard`); its grants name concrete owners and resources. A
 * {@link PolymorphicPermissionCard} (`WitTypes.PolymorphicPermissionCard`) is a template card
 * whose grants may contain slot variables (`?agent`, `?env`, ...), which resolve for each holder.
 * Both support the same operations, and the SDK has no conversion API between them; a card that a
 * schema decodes takes the type that the schema declares. Borrowing operations
 * (`id`, `parents`, `expiresAt`, `inspect`, `derive`, `deriveScope`) leave the card usable.
 * Consuming operations (`revoke`, `install`, `release`) move the card before they call the host,
 * because the host drops the handle even when the call fails; any later use of a moved card fails
 * with {@link PermissionsHostError}.
 *
 * A {@link ScopeCard} is a separate, invocation-local card from {@link deriveScope}, or the scope
 * card lent to the current invocation, listed by {@link wallet}. Only scope cards attach to
 * awaited agent calls. A scope card supports `id`, `parents`, `expiresAt`,
 * `inspect`, `release`, and use as a `deriveScope` parent. It has no schema codec, so it cannot
 * be an agent-method argument or result, and it cannot be revoked, installed, or derived from.
 * It is valid only during the invocation that derived or received it.
 *
 * Cards cannot be constructed, parsed, or deserialized by user code. Grants, bounds, views, and
 * card identifiers are plain data: the host validates every derivation request against the
 * parent card or the caller's wallet.
 *
 * @since 1.6.0
 */
import { Effect, Equal, Hash, Option, Schema, Scope } from "effect"
import * as Duration from "effect/Duration"
import { NodeInspectSymbol } from "effect/Inspectable"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid } from "golem:core/types@2.0.0"
import type * as KernelHost from "golem:permissions/kernel-introspection@0.1.0"
import type * as TypesHost from "golem:permissions/types@0.1.0"
import type { Identity } from "./AgentIdentity.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { PermissionsClient } from "./host/PermissionsClient.js"
import { assertCapabilityReady } from "./internal/schema-model/capabilityTransaction.js"
import {
  createGuestPermissionCardHandle,
  type GuestPermissionCardHandle,
  isGuestPermissionCardHandle,
  type PermissionCardKind,
  peekGuestPermissionCardHandle,
  releaseGuestPermissionCardHandle,
  takeGuestPermissionCardHandle,
} from "./internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "./internal/schema-model/permissionCardInternal.js"
import {
  createScopeCard,
  type GuestScopeCard,
  isScopeCard,
  peekScopeCard,
  releaseScopeCard,
} from "./internal/scopeCard.js"

/**
 * An owned, opaque concrete permission card: every grant names concrete owners and resources. It
 * is the same value that `WitTypes.PermissionCard` schemas (`polymorphic: false`) decode and
 * encode, so cards returned here can be passed to agent methods, tools, and middleware, and
 * cards received there can be managed here.
 *
 * @since 1.6.0
 * @category models
 */
export type PermissionCard = GuestPermissionCardHandle<"concrete">

/**
 * An owned, opaque template permission card: its grants may contain slot variables (`?agent`,
 * `?env`, ...), which resolve for each holder. It is the same value that
 * `WitTypes.PolymorphicPermissionCard` schemas (`polymorphic: true`) decode and encode. It
 * supports the same operations as a {@link PermissionCard}; {@link derive} from it returns a
 * {@link PermissionCard}, because the host resolves the template for the holder.
 *
 * @since 1.6.0
 * @category models
 */
export type PolymorphicPermissionCard = GuestPermissionCardHandle<"template">

/**
 * A card that can be derived from, revoked, and installed.
 *
 * @since 1.6.0
 * @category models
 */
export type PersistentCard = PermissionCard | PolymorphicPermissionCard

/**
 * An invocation-local scope card from {@link deriveScope}, or the scope card lent to the current
 * invocation (a `Scope` entry of {@link wallet}). Attach it to awaited agent calls with
 * `withScopeCard` or a client's `scopeCard` option; each call borrows it. It is valid only during
 * the invocation that derived or received it: keeping it, or a client whose default is it, for a later
 * invocation makes the host deny awaited calls with `denied`. It is not a {@link PermissionCard}
 * and has no schema or wire codec.
 *
 * @since 1.6.0
 * @category models
 */
export type ScopeCard = GuestScopeCard

/**
 * Any card that borrowing operations accept.
 *
 * @since 1.6.0
 * @category models
 */
export type AnyCard = PersistentCard | ScopeCard

/**
 * A card in the caller's wallet, labelled with its kind. Each kind has its own card type.
 *
 * @since 1.6.0
 * @category models
 */
export type WalletCard =
  | { readonly _tag: "Concrete"; readonly card: PermissionCard }
  | { readonly _tag: "Template"; readonly card: PolymorphicPermissionCard }
  | { readonly _tag: "Scope"; readonly card: ScopeCard }

/**
 * One grant pattern: `class(owner) @ recipient : verb : resourceId`.
 *
 * @since 1.6.0
 * @category schemas
 */
export const PatternGrant = Schema.Struct({
  class: Schema.String,
  owner: Schema.String,
  recipient: Schema.String,
  verb: Schema.String,
  resourceId: Schema.String,
})

/**
 * One grant pattern: `class(owner) @ recipient : verb : resourceId`.
 *
 * @since 1.6.0
 * @category models
 */
export type PatternGrant = typeof PatternGrant.Type

/**
 * The four grant lists of a card's lattice.
 *
 * @since 1.6.0
 * @category schemas
 */
export const CardView = Schema.Struct({
  lowerPositive: Schema.Array(PatternGrant),
  lowerNegative: Schema.Array(PatternGrant),
  upperPositive: Schema.Array(PatternGrant),
  upperNegative: Schema.Array(PatternGrant),
})

/**
 * The four grant lists of a card's lattice.
 *
 * @since 1.6.0
 * @category models
 */
export type CardView = typeof CardView.Type

/**
 * Requested lattice bounds. Omitted lists are empty.
 *
 * For {@link derive}, the positive lists name the parent grants to retain and the negative lists
 * name the denials to add. For {@link deriveFromWallet} and {@link deriveScope}, the four lists
 * are the complete requested lattice.
 *
 * @since 1.6.0
 * @category models
 */
export interface Bounds {
  readonly lowerPositive?: ReadonlyArray<PatternGrant>
  readonly lowerNegative?: ReadonlyArray<PatternGrant>
  readonly upperPositive?: ReadonlyArray<PatternGrant>
  readonly upperNegative?: ReadonlyArray<PatternGrant>
}

/**
 * A card lifespan, sent to the host as an ISO-8601 duration of the form `PnDTnHnMnS`.
 *
 * @since 1.6.0
 * @category models
 */
export class Lifespan {
  private constructor(readonly iso8601: string) {}

  /**
   * Use an ISO-8601 duration string in days and time units, such as `"P30D"` or `"PT1H30M"`. The
   * host parses and validates the value; it rejects calendar units (years, months, weeks) and a
   * malformed value fails the derivation with an `invalid-pattern` {@link PermissionError}.
   *
   * @throws TypeError when the value does not start with `P`.
   * @since 1.6.0
   * @category constructors
   */
  static fromIso8601(iso8601: string): Lifespan {
    if (!iso8601.startsWith("P")) {
      throw new TypeError(`an ISO-8601 lifespan must start with 'P': ${iso8601}`)
    }
    return new Lifespan(iso8601)
  }

  /**
   * Use a finite, positive Effect duration. It is sent as whole and fractional seconds
   * (`PT<n>S`).
   *
   * @throws TypeError when the duration is infinite, zero, or negative.
   * @since 1.6.0
   * @category constructors
   */
  static fromDuration(input: Duration.Input): Lifespan {
    const duration = Duration.fromInputUnsafe(input)
    if (!Duration.isFinite(duration)) throw new TypeError("a lifespan must be a finite duration")
    const nanos = Duration.toNanosUnsafe(duration)
    if (nanos <= 0n) throw new TypeError("a lifespan must be a positive duration")
    const seconds = nanos / 1_000_000_000n
    const fraction = (nanos % 1_000_000_000n).toString().padStart(9, "0").replace(/0+$/, "")
    return new Lifespan(`PT${seconds}${fraction === "" ? "" : `.${fraction}`}S`)
  }
}

/**
 * A host timestamp with nanosecond precision.
 *
 * @since 1.6.0
 * @category models
 */
export interface Timestamp {
  readonly seconds: bigint
  readonly nanoseconds: number
}

/**
 * Identifies a wallet's current card set.
 *
 * @since 1.6.0
 * @category models
 */
export interface WalletVersionToken {
  readonly walletIdHash: Uint8Array
  readonly generation: bigint
}

/**
 * A resource class known to the permission kernel and its verbs.
 *
 * @since 1.6.0
 * @category models
 */
export interface KernelModule {
  readonly className: string
  readonly verbs: ReadonlyArray<string>
}

// Card IDs exist only in the bundle that contains this module, so the keys stay module-local
// and user code cannot read them.
const cardIdKeys = new WeakMap<object, string>()
const redactedCardId = "CardId(<redacted>)"

/**
 * An opaque card identifier. It supports equality and hashing through `Equal.equals` and
 * Effect collections only; it has no public constructor and exposes no UUID or string form.
 * Printing, logging, and JSON serialization show a redacted placeholder. Use it to compare
 * {@link id} with {@link parents}.
 *
 * @since 1.6.0
 * @category models
 */
export class CardId implements Equal.Equal {
  private constructor() {
    throw new TypeError("card IDs cannot be constructed")
  }

  /** @since 1.6.0 */
  [Equal.symbol](that: Equal.Equal): boolean {
    const key = cardIdKeys.get(this)
    return key !== undefined && key === cardIdKeys.get(that)
  }

  /** @since 1.6.0 */
  [Hash.symbol](): number {
    return Hash.string(cardIdKeys.get(this) ?? "")
  }

  /** @since 1.6.0 */
  toString(): string {
    return redactedCardId
  }

  /** @since 1.6.0 */
  toJSON(): string {
    return redactedCardId
  }

  /** @since 1.6.0 */
  [NodeInspectSymbol](): string {
    return redactedCardId
  }
}

const makeCardId = (raw: TypesHost.CardId): CardId => {
  const id = Object.create(CardId.prototype) as CardId
  cardIdKeys.set(id, `${raw.uuid.highBits.toString(16)}-${raw.uuid.lowBits.toString(16)}`)
  return Object.freeze(id)
}

/**
 * The target of {@link install}. The host currently accepts only agent holders; account and app
 * holders fail with a `not-permitted` {@link PermissionError}.
 *
 * @since 1.6.0
 * @category models
 */
export type Holder =
  | { readonly _tag: "Account"; readonly accountId: string }
  | { readonly _tag: "App"; readonly appId: string }
  | { readonly _tag: "Agent"; readonly agentId: CoreTypes.AgentId }

const holderTags = {
  account: "Account",
  app: "App",
  agent: "Agent",
} as const satisfies Record<TypesHost.Holder["tag"], Holder["_tag"]>

/**
 * Holder constructors.
 *
 * @since 1.6.0
 * @category constructors
 */
export const Holder = {
  /** An account holder, identified by its canonical UUID string. */
  account: (accountId: string): Holder => ({ _tag: holderTags.account, accountId }),
  /** An application holder, identified by its canonical UUID string. */
  app: (appId: string): Holder => ({ _tag: holderTags.app, appId }),
  /** An agent holder, such as the value of `SelfAgentId.SelfAgentId`. */
  agent: (agentId: CoreTypes.AgentId): Holder => ({ _tag: holderTags.agent, agentId }),
}

/**
 * The tag of each `permission-error` case of `golem:permissions/types@0.1.0`.
 *
 * @since 1.6.0
 * @category models
 */
export type PermissionErrorKind = TypesHost.PermissionError["tag"]

const permissionErrorKindSet = {
  "not-permitted": true,
  "lower-bound-too-broad": true,
  "upper-bound-too-broad": true,
  "wallet-multi-source-required": true,
  "card-revoked": true,
  "card-expired": true,
  "install-recipient-mismatch": true,
  "unknown-resource-class": true,
  "invalid-pattern": true,
  "invalid-verb": true,
  "invalid-owner": true,
  "invalid-recipient": true,
} as const satisfies Record<PermissionErrorKind, true>

/**
 * Every {@link PermissionErrorKind}, in WIT order.
 *
 * @since 1.6.0
 * @category models
 */
export const permissionErrorKinds: ReadonlyArray<PermissionErrorKind> = Object.freeze(
  Object.keys(permissionErrorKindSet) as PermissionErrorKind[],
)

/**
 * An expected failure reported by the permission host.
 *
 * @since 1.6.0
 * @category errors
 */
export class PermissionError {
  /** @since 1.6.0 */
  readonly _tag = "PermissionError"
  /** @since 1.6.0 */
  readonly message: string
  constructor(
    readonly kind: PermissionErrorKind,
    readonly detail: string,
  ) {
    this.message = `PermissionError(${kind}): ${detail}`
  }
}

/**
 * A failure outside the host's `permission-error` contract: a moved or invalid card handle, an
 * invalid holder, or an unexpected host exception.
 *
 * @since 1.6.0
 * @category errors
 */
export class PermissionsHostError {
  /** @since 1.6.0 */
  readonly _tag = "PermissionsHostError"
  /** @since 1.6.0 */
  readonly message: string
  constructor(
    readonly operation: string,
    readonly cause: unknown,
  ) {
    this.message = `PermissionsHostError(${operation}): ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

const isWitPermissionError = (value: unknown): value is TypesHost.PermissionError => {
  if (value === null || typeof value !== "object") return false
  const { tag, val } = value as { tag?: unknown; val?: unknown }
  return (
    typeof tag === "string" && Object.hasOwn(permissionErrorKindSet, tag) && typeof val === "string"
  )
}

const extractPermissionError = (error: unknown): TypesHost.PermissionError | undefined => {
  if (isWitPermissionError(error)) return error
  if (error !== null && typeof error === "object") {
    const { payload, cause } = error as { payload?: unknown; cause?: unknown }
    if (isWitPermissionError(payload)) return payload
    if (isWitPermissionError(cause)) return cause
  }
  return undefined
}

const failure =
  (operation: string) =>
  (cause: unknown): PermissionError | PermissionsHostError => {
    const error = extractPermissionError(cause)
    return error === undefined
      ? new PermissionsHostError(operation, cause)
      : new PermissionError(error.tag, error.val)
  }

const hostFailure =
  (operation: string) =>
  (cause: unknown): PermissionsHostError =>
    new PermissionsHostError(operation, cause)

const requireCard = (card: PersistentCard): PersistentCard => {
  if (!isGuestPermissionCardHandle(card)) throw new TypeError("not a permission card")
  assertCapabilityReady(card)
  return card
}

const borrow = (card: PersistentCard): CoreTypes.PermissionCard => {
  const raw = peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, requireCard(card))
  if (raw === undefined) throw new Error("permission card has already been moved")
  return raw
}

const borrowAny = (card: AnyCard): CoreTypes.PermissionCard => {
  if (!isScopeCard(card)) return borrow(card)
  const raw = peekScopeCard(card)
  if (raw === undefined) throw new Error("scope card has already been released")
  return raw
}

const take = (card: PersistentCard): CoreTypes.PermissionCard => {
  const raw = takeGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, requireCard(card))
  if (raw === undefined) throw new Error("permission card has already been moved")
  return raw
}

const wrap = <K extends PermissionCardKind>(
  raw: CoreTypes.PermissionCard,
  kind: K,
): GuestPermissionCardHandle<K> =>
  createGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, raw, kind)

const wrapWalletCard = (entry: TypesHost.WalletCard): WalletCard => {
  switch (entry.tag) {
    case "concrete":
      return { _tag: "Concrete", card: wrap(entry.val, "concrete") }
    case "template":
      return { _tag: "Template", card: wrap(entry.val, "template") }
    case "scope":
      return { _tag: "Scope", card: createScopeCard(entry.val) }
  }
}

const toWitGrant = (grant: PatternGrant): TypesHost.PatternGrant => ({
  class_: grant.class,
  owner: grant.owner,
  recipient: grant.recipient,
  verb: grant.verb,
  resourceId: grant.resourceId,
})

const fromWitGrant = (grant: TypesHost.PatternGrant): PatternGrant => ({
  class: grant.class_,
  owner: grant.owner,
  recipient: grant.recipient,
  verb: grant.verb,
  resourceId: grant.resourceId,
})

const toWitGrants = (grants: ReadonlyArray<PatternGrant> | undefined): TypesHost.PatternGrant[] =>
  (grants ?? []).map(toWitGrant)

const toWitLifespan = (lifespan: Lifespan | undefined): TypesHost.Duration | undefined => {
  if (lifespan === undefined) return undefined
  if (!(lifespan instanceof Lifespan)) throw new TypeError("not a Lifespan")
  return { iso8601: lifespan.iso8601 }
}

const toWitHolder = (holder: Holder): TypesHost.Holder => {
  switch (holder._tag) {
    case "Account":
      return { tag: "account", val: parseUuid(holder.accountId) }
    case "App":
      return { tag: "app", val: parseUuid(holder.appId) }
    case "Agent":
      return { tag: "agent", val: holder.agentId }
  }
}

/**
 * The card's identifier. The host traps when the handle is unknown or revoked.
 *
 * @since 1.6.0
 * @category accessors
 */
export const id = (card: AnyCard): Effect.Effect<CardId, PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () => makeCardId(host.id(borrowAny(card))),
      catch: hostFailure("id"),
    })
  })

/**
 * The identifiers of the card's parents in the derivation graph. The host traps when the handle
 * is unknown or revoked.
 *
 * @since 1.6.0
 * @category accessors
 */
export const parents = (
  card: AnyCard,
): Effect.Effect<ReadonlyArray<CardId>, PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () => host.parents(borrowAny(card)).map(makeCardId),
      catch: hostFailure("parents"),
    })
  })

/**
 * The card's expiry, if it has a bounded lifespan. For a scope card, the earliest expiry of the
 * root cards of this scope card, or none when no root card expires. The scope card stops
 * authorizing then. The host traps when the handle is unknown or revoked.
 *
 * @since 1.6.0
 * @category accessors
 */
export const expiresAt = (
  card: AnyCard,
): Effect.Effect<Option.Option<Timestamp>, PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () => {
        const timestamp = host.expiresAt(borrowAny(card))
        return timestamp === undefined
          ? Option.none()
          : Option.some({ seconds: timestamp.seconds, nanoseconds: timestamp.nanoseconds })
      },
      catch: hostFailure("expiresAt"),
    })
  })

/**
 * The card's complete grant lattice. Requires `card:inspect` authority.
 *
 * @since 1.6.0
 * @category accessors
 */
export const inspect = (
  card: AnyCard,
): Effect.Effect<CardView, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () => {
        const view = host.inspect(borrowAny(card))
        return {
          lowerPositive: view.lowerPositive.map(fromWitGrant),
          lowerNegative: view.lowerNegative.map(fromWitGrant),
          upperPositive: view.upperPositive.map(fromWitGrant),
          upperNegative: view.upperNegative.map(fromWitGrant),
        }
      },
      catch: failure("inspect"),
    })
  })

/**
 * Derive a narrower child card from `card`, which stays usable. The positive bounds name the
 * parent grants to retain; the negative bounds name the denials to add. A template parent is
 * resolved against the caller, so the result is always a concrete {@link PermissionCard}.
 * Requires `card:derive` authority.
 *
 * @since 1.6.0
 * @category constructors
 */
export const derive = (
  card: PersistentCard,
  bounds: Bounds,
  lifespan?: Lifespan,
): Effect.Effect<PermissionCard, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () =>
        wrap(
          host.derive(
            borrow(card),
            toWitGrants(bounds.lowerPositive),
            toWitGrants(bounds.lowerNegative),
            toWitGrants(bounds.upperPositive),
            toWitGrants(bounds.upperNegative),
            toWitLifespan(lifespan),
          ),
          "concrete",
        ),
      catch: failure("derive"),
    })
  })

/**
 * Derive a card directly from the caller's wallet. The bounds are the complete requested
 * lattice.
 *
 * @since 1.6.0
 * @category constructors
 */
export const deriveFromWallet = (
  bounds: Bounds,
  lifespan?: Lifespan,
): Effect.Effect<PermissionCard, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () =>
        wrap(
          host.deriveFromWallet(
            toWitGrants(bounds.lowerPositive),
            toWitGrants(bounds.lowerNegative),
            toWitGrants(bounds.upperPositive),
            toWitGrants(bounds.upperNegative),
            toWitLifespan(lifespan),
          ),
          "concrete",
        ),
      catch: failure("deriveFromWallet"),
    })
  })

/**
 * Derive an invocation-local {@link ScopeCard} from one or more persistent or scope parents,
 * which stay usable. The bounds are the complete requested lattice. Attach the result to an
 * awaited agent call with `withScopeCard` or the client's `scopeCard` option. The scope card is
 * valid only during the current invocation.
 *
 * @since 1.6.0
 * @category constructors
 */
export const deriveScope = (
  parents: ReadonlyArray<AnyCard>,
  bounds: Bounds,
): Effect.Effect<ScopeCard, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () =>
        createScopeCard(
          host.deriveScope(
            parents.map(borrowAny),
            toWitGrants(bounds.lowerPositive),
            toWitGrants(bounds.lowerNegative),
            toWitGrants(bounds.upperPositive),
            toWitGrants(bounds.upperNegative),
          ),
        ),
      catch: failure("deriveScope"),
    })
  })

/**
 * Revoke `card` and every card derived from it, and return the number of revoked cards. The
 * card is moved before the host call, so it is unusable afterwards even when revocation fails.
 *
 * @since 1.6.0
 * @category consuming
 */
export const revoke = (
  card: PersistentCard,
): Effect.Effect<number, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    return yield* Effect.try({
      try: () => host.revoke(take(card)),
      catch: failure("revoke"),
    })
  })

/**
 * Install `card` into the wallet of `target`. The card is moved before the host call, so it is
 * unusable afterwards even when installation fails. The host accepts only agent holders, and
 * fails with `install-recipient-mismatch` when the card's recipient does not admit the target.
 *
 * @since 1.6.0
 * @category consuming
 */
export const install = (
  card: PersistentCard,
  target: Holder,
): Effect.Effect<void, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    const holder = yield* Effect.try({
      try: () => toWitHolder(target),
      catch: hostFailure("install"),
    })
    yield* Effect.try({
      try: () => host.install(take(card), holder),
      catch: failure("install"),
    })
  })

/**
 * Drop a card or scope card that the caller does not keep. Releasing a released card does
 * nothing. Releasing a card that is being sent in an awaited call does nothing at once; if that
 * call never reaches the host, the SDK drops the card instead of giving it back.
 *
 * @since 1.6.0
 * @category consuming
 */
export const release = (
  card: AnyCard,
): Effect.Effect<void, PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    yield* Effect.try({
      try: () => {
        const raw = isScopeCard(card)
          ? releaseScopeCard(card)
          : releaseGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, requireCard(card))
        if (raw !== undefined) host.drop(raw)
      },
      catch: hostFailure("release"),
    })
  })

/**
 * Every card installed in the caller's wallet, as owned handles labelled with their kind: a
 * `Concrete` {@link PermissionCard}, a `Template` {@link PolymorphicPermissionCard}, or a `Scope`
 * {@link ScopeCard}. Release cards that are not kept with {@link release}, or use
 * {@link walletScoped}.
 *
 * While an invocation runs with a scope card lent by its caller, the list also contains that
 * scope card as a `Scope` entry if the agent holds `card:inspect`. It is retired when the
 * invocation ends.
 *
 * @since 1.6.0
 * @category wallet
 */
export const wallet: Effect.Effect<
  ReadonlyArray<WalletCard>,
  PermissionsHostError,
  PermissionsClient
> = Effect.gen(function* () {
  const host = yield* PermissionsClient
  return yield* Effect.try({
    try: () => host.wallet().map(wrapWalletCard),
    catch: hostFailure("wallet"),
  })
})

/**
 * Like {@link wallet}, but releases every returned card that is still held when the scope
 * closes. Cards moved by `revoke`, `install`, or schema transfer are not affected.
 *
 * @since 1.6.0
 * @category wallet
 */
export const walletScoped: Effect.Effect<
  ReadonlyArray<WalletCard>,
  PermissionsHostError,
  PermissionsClient | Scope.Scope
> = Effect.acquireRelease(wallet, (cards) =>
  Effect.forEach(cards, (entry) => Effect.ignore(release(entry.card)), { discard: true }),
)

/**
 * The version token of the caller's wallet. It changes whenever the installed card set changes.
 *
 * @since 1.6.0
 * @category wallet
 */
export const walletVersion: Effect.Effect<
  WalletVersionToken,
  PermissionsHostError,
  PermissionsClient
> = Effect.gen(function* () {
  const host = yield* PermissionsClient
  return yield* Effect.try({
    try: () => {
      const token = host.walletVersion()
      return { walletIdHash: token.walletIdHash, generation: token.generation }
    },
    catch: hostFailure("walletVersion"),
  })
})

/**
 * The agent holder for a parsed agent identity, resolved through the registered agent type.
 *
 * @since 1.6.0
 * @category constructors
 */
export const agentHolder = (
  identity: Identity,
): Effect.Effect<Holder, PermissionsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const host = yield* AgentHostClient
    return yield* Effect.try({
      try: () => {
        const registered = host.getAgentType(identity.typeName)
        if (registered === undefined)
          throw new Error(`agent type '${identity.typeName}' is not registered`)
        return Holder.agent({ componentId: registered.implementedBy, agentId: identity.encoded })
      },
      catch: hostFailure("agentHolder"),
    })
  })

/**
 * Every resource class the permission kernel knows, with its verbs.
 *
 * @since 1.6.0
 * @category kernel
 */
export const listModules: Effect.Effect<
  ReadonlyArray<KernelModule>,
  PermissionsHostError,
  PermissionsClient
> = Effect.gen(function* () {
  const host = yield* PermissionsClient
  return yield* Effect.try({
    try: () =>
      host.listModules().map((module: KernelHost.KernelModule) => ({
        className: module.className,
        verbs: [...module.verbs],
      })),
    catch: hostFailure("listModules"),
  })
})

/**
 * Check that a grant names a known class, owner, recipient, verb, and resource pattern.
 *
 * @since 1.6.0
 * @category kernel
 */
export const validateGrant = (
  grant: PatternGrant,
): Effect.Effect<void, PermissionError | PermissionsHostError, PermissionsClient> =>
  Effect.gen(function* () {
    const host = yield* PermissionsClient
    yield* Effect.try({
      try: () => host.validateGrant(toWitGrant(grant)),
      catch: failure("validateGrant"),
    })
  })
