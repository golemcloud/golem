// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Typed permission-card management over `golem:permissions@0.1.0`.
//
// A `PermissionCard` wraps the SDK's take-once `GuestPermissionCardHandle`
// carrier. Borrowing operations peek the handle; consuming operations (`revoke`,
// `install`, `release`) take it before calling the host, because the host drops
// the owned handle even when the call fails. Nothing in this module turns an
// identifier or plain data into a card: cards only come from the host.

import type {
  CardId as RawCardId,
  PermissionCard as RawPermissionCard,
  Uuid as RawUuid,
} from 'golem:core/types@2.0.0';
import {
  expiresAt as rawExpiresAt,
  id as rawId,
  parents as rawParents,
  type CardView as RawCardView,
  type Duration as RawDuration,
  type Holder as RawHolder,
  type PatternGrant as RawPatternGrant,
  type PermissionError as RawPermissionError,
} from 'golem:permissions/types@0.1.0';
import { inspectCard } from 'golem:permissions/inspect@0.1.0';
import {
  derive as rawDerive,
  deriveFromWallet as rawDeriveFromWallet,
  deriveScope as rawDeriveScope,
} from 'golem:permissions/derive@0.1.0';
import { revokeCard } from 'golem:permissions/revoke@0.1.0';
import { installCard, selfVersion, selfWallet } from 'golem:permissions/wallet@0.1.0';
import {
  listModules as rawListModules,
  validateGrant as rawValidateGrant,
} from 'golem:permissions/kernel-introspection@0.1.0';
import { type SchemaValue, v } from '../internal/schema-model/model';
import {
  bindGuestPermissionCardHandleToCard,
  registerPermissionCardObject,
  createGuestPermissionCardHandle,
  GuestPermissionCardHandle,
  guestPermissionCardHandleIsBoundToCard,
  permissionCardHandleWasLiftedFromWire,
  releaseGuestPermissionCardHandle,
  peekGuestPermissionCardHandle,
  takeGuestPermissionCardHandle,
} from '../internal/schema-model/permissionCardHandle';
import {
  PERMISSION_CARD_INTERNAL,
  type PermissionCardInternal,
} from '../internal/schema-model/permissionCardInternal';
import type { AgentId } from './hostapi';
import type { Duration } from './retryBuilder';
import { Result } from './result';
import type { Uuid } from '../uuid';

// ============================================================
// Plain data
// ============================================================

/**
 * One grant pattern: `class(owner) @ recipient : verb : resource-id`.
 *
 * Plain data. A grant never carries authority by itself; derive inputs are
 * requests that the host validates against the parent card or the wallet.
 */
export interface PatternGrant {
  readonly class: string;
  readonly owner: string;
  readonly recipient: string;
  readonly verb: string;
  readonly resourceId: string;
}

/** The four grant lists of a card's bound lattice. */
export interface CardView {
  readonly lowerPositive: readonly PatternGrant[];
  readonly lowerNegative: readonly PatternGrant[];
  readonly upperPositive: readonly PatternGrant[];
  readonly upperNegative: readonly PatternGrant[];
}

/**
 * The four grant lists of a derive request. An omitted list is empty.
 *
 * For {@link PermissionCard.derive} the positive lists name the parent grants
 * to retain and the negative lists name the denials to add. For
 * {@link deriveFromWallet} and {@link deriveScope} they are the full requested
 * lists.
 */
export interface Bounds {
  readonly lowerPositive?: readonly PatternGrant[];
  readonly lowerNegative?: readonly PatternGrant[];
  readonly upperPositive?: readonly PatternGrant[];
  readonly upperNegative?: readonly PatternGrant[];
}

/** An instant: seconds and nanoseconds since the Unix epoch (UTC). */
export interface Timestamp {
  readonly seconds: bigint;
  readonly nanoseconds: number;
}

/** The caller's wallet version. It changes whenever the set of installed cards changes. */
export interface WalletVersionToken {
  /** 32 bytes: BLAKE3 of (holder kind, holder id). */
  readonly walletIdHash: Uint8Array;
  readonly generation: bigint;
}

/** A resource class registered in the permission kernel, with its verbs. */
export interface KernelModule {
  readonly className: string;
  readonly verbs: readonly string[];
}

/**
 * The lifespan of a derived card, as an ISO-8601 duration.
 *
 * The host accepts only the `PnDTnHnMnS` form (days, hours, minutes and
 * seconds; for example `"P1DT2H"` or `"PT30S"`) and rejects other values,
 * including year, month and week units, with `invalid-pattern`.
 */
export class Lifespan {
  private constructor(readonly iso8601: string) {
    Object.freeze(this);
  }

  /** A lifespan from an ISO-8601 duration string such as `"PT1H"`. */
  static iso8601(value: string): Lifespan {
    if (typeof value !== 'string') {
      throw new TypeError('Lifespan.iso8601 expects an ISO-8601 duration string');
    }
    return new Lifespan(value);
  }

  /** A lifespan from a positive SDK {@link Duration}. */
  static of(duration: Duration): Lifespan {
    const nanoseconds = duration.asNanoseconds();
    if (nanoseconds <= 0n) {
      throw new RangeError('a card lifespan must be a positive duration');
    }
    const seconds = nanoseconds / 1_000_000_000n;
    const fraction = nanoseconds % 1_000_000_000n;
    const fractionText =
      fraction === 0n ? '' : `.${fraction.toString().padStart(9, '0').replace(/0+$/, '')}`;
    return new Lifespan(`PT${seconds}${fractionText}S`);
  }
}

const cardIdKeys = new WeakMap<CardId, string>();
const internedCardIds = new Map<string, { deref(): CardId | undefined }>();
const cardIdRegistry =
  typeof FinalizationRegistry === 'function'
    ? new FinalizationRegistry<string>((key) => {
        if (internedCardIds.get(key)?.deref() === undefined) internedCardIds.delete(key);
      })
    : undefined;
const CARD_ID_INTERNAL: unique symbol = Symbol('golem:card-id internal capability');

const CARD_ID_PLACEHOLDER = 'CardId(<redacted>)';

/**
 * An opaque permission-card identifier.
 *
 * Card IDs are host-private: a `CardId` has no public constructor and exposes
 * no UUID or string form. Printing or serializing one gives a redacted
 * placeholder. The SDK interns identifiers, so two `CardId` values for the same
 * card are the same object; compare them with `===` or {@link CardId.equals},
 * and use them as `Map` / `Set` keys.
 */
export class CardId {
  /** @internal */
  constructor(key: typeof CARD_ID_INTERNAL, value: string) {
    if (key !== CARD_ID_INTERNAL) {
      throw new Error('CardId construction is an internal SDK operation');
    }
    cardIdKeys.set(this, value);
    Object.freeze(this);
  }

  equals(other: CardId): boolean {
    const key = cardIdKeys.get(this);
    return key !== undefined && key === cardIdKeys.get(other);
  }

  toString(): string {
    return CARD_ID_PLACEHOLDER;
  }

  toJSON(): string {
    return CARD_ID_PLACEHOLDER;
  }

  [Symbol.for('nodejs.util.inspect.custom')](): string {
    return CARD_ID_PLACEHOLDER;
  }
}

function cardIdFromRaw(raw: RawCardId): CardId {
  const key = `${raw.uuid.highBits}:${raw.uuid.lowBits}`;
  const existing = internedCardIds.get(key)?.deref();
  if (existing !== undefined) return existing;
  const created = new CardId(CARD_ID_INTERNAL, key);
  if (cardIdRegistry !== undefined && typeof WeakRef === 'function') {
    internedCardIds.set(key, new WeakRef(created));
    cardIdRegistry.register(created, key);
  } else {
    internedCardIds.set(key, { deref: () => created });
  }
  return created;
}

/**
 * The target of {@link PermissionCard.install}.
 *
 * The host currently installs cards only on agents; account and application
 * targets return `not-permitted`.
 */
export class Holder {
  private constructor(
    readonly tag: 'account' | 'app' | 'agent',
    readonly val: Uuid | AgentId,
  ) {
    Object.freeze(this);
  }

  static account(accountId: Uuid): Holder {
    return new Holder('account', accountId);
  }

  static app(applicationId: Uuid): Holder {
    return new Holder('app', applicationId);
  }

  static agent(agentId: AgentId): Holder {
    return new Holder('agent', agentId);
  }
}

// ============================================================
// Errors
// ============================================================

/** The case names of the WIT `permission-error` variant. */
export type PermissionErrorKind = RawPermissionError['tag'];

/** Every {@link PermissionErrorKind}, in WIT declaration order. */
export const PERMISSION_ERROR_KINDS = Object.freeze([
  'not-permitted',
  'lower-bound-too-broad',
  'upper-bound-too-broad',
  'wallet-multi-source-required',
  'card-revoked',
  'card-expired',
  'install-recipient-mismatch',
  'unknown-resource-class',
  'invalid-pattern',
  'invalid-verb',
  'invalid-owner',
  'invalid-recipient',
] as const satisfies readonly PermissionErrorKind[]);

// Fails to typecheck when the WIT variant gains a case this list does not name.
const permissionErrorKindsAreExhaustive: [
  Exclude<PermissionErrorKind, (typeof PERMISSION_ERROR_KINDS)[number]>,
] extends [never]
  ? true
  : never = true;
void permissionErrorKindsAreExhaustive;

const permissionErrorKinds: ReadonlySet<string> = new Set(PERMISSION_ERROR_KINDS);

/** An expected failure of a permission-card operation, reported by the host. */
export class PermissionError extends Error {
  readonly _tag = 'PermissionError';

  constructor(
    readonly kind: PermissionErrorKind,
    readonly details: string,
  ) {
    super(`${kind}: ${details}`);
    this.name = 'PermissionError';
  }
}

function permissionErrorFromHost(error: unknown): PermissionError | undefined {
  if (typeof error !== 'object' || error === null) return undefined;
  const { tag, val } = error as { tag?: unknown; val?: unknown };
  if (typeof tag !== 'string' || !permissionErrorKinds.has(tag) || typeof val !== 'string') {
    return undefined;
  }
  return new PermissionError(tag as PermissionErrorKind, val);
}

function hostCall<T>(operation: () => T): Result<T, PermissionError> {
  try {
    return Result.ok(operation());
  } catch (error) {
    const permissionError = permissionErrorFromHost(error);
    if (permissionError === undefined) throw error;
    return Result.err(permissionError);
  }
}

// ============================================================
// Conversions
// ============================================================

function grantToRaw(grant: PatternGrant): RawPatternGrant {
  return {
    class_: grant.class,
    owner: grant.owner,
    recipient: grant.recipient,
    verb: grant.verb,
    resourceId: grant.resourceId,
  };
}

function grantFromRaw(raw: RawPatternGrant): PatternGrant {
  return Object.freeze({
    class: raw.class_,
    owner: raw.owner,
    recipient: raw.recipient,
    verb: raw.verb,
    resourceId: raw.resourceId,
  });
}

function grantsToRaw(grants: readonly PatternGrant[] | undefined): RawPatternGrant[] {
  return (grants ?? []).map(grantToRaw);
}

function cardViewFromRaw(raw: RawCardView): CardView {
  return Object.freeze({
    lowerPositive: Object.freeze(raw.lowerPositive.map(grantFromRaw)),
    lowerNegative: Object.freeze(raw.lowerNegative.map(grantFromRaw)),
    upperPositive: Object.freeze(raw.upperPositive.map(grantFromRaw)),
    upperNegative: Object.freeze(raw.upperNegative.map(grantFromRaw)),
  });
}

function lifespanToRaw(lifespan: Lifespan | undefined): RawDuration | undefined {
  if (lifespan === undefined) return undefined;
  if (!(lifespan instanceof Lifespan)) {
    throw new TypeError('expected a Lifespan; build one with Lifespan.iso8601 or Lifespan.of');
  }
  return { iso8601: lifespan.iso8601 };
}

function uuidToRaw(uuid: Uuid): RawUuid {
  return { highBits: uuid.highBits, lowBits: uuid.lowBits };
}

function holderToRaw(holder: Holder): RawHolder {
  if (!(holder instanceof Holder)) {
    throw new TypeError(
      'expected a Holder; build one with Holder.agent, Holder.account or Holder.app',
    );
  }
  if (holder.tag === 'agent') {
    const agentId = holder.val as AgentId;
    return {
      tag: 'agent',
      val: { componentId: { uuid: uuidToRaw(agentId.componentId.uuid) }, agentId: agentId.agentId },
    };
  }
  return { tag: holder.tag, val: uuidToRaw(holder.val as Uuid) };
}

// ============================================================
// Permission card
// ============================================================

const cardHandles = new WeakMap<PermissionCard, GuestPermissionCardHandle>();

const CARD_MOVED =
  'permission card has already been moved (revoked, installed, released, or sent to another ' +
  'agent) and can no longer be used';

function handleOf(card: PermissionCard): GuestPermissionCardHandle {
  const handle = cardHandles.get(card);
  if (handle === undefined) {
    throw new Error('invalid permission card');
  }
  return handle;
}

function borrow(card: PermissionCard): RawPermissionCard {
  const raw = peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handleOf(card));
  if (raw === undefined) throw new Error(CARD_MOVED);
  return raw;
}

function take(card: PermissionCard): RawPermissionCard {
  const raw = takeGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handleOf(card));
  if (raw === undefined) throw new Error(CARD_MOVED);
  return raw;
}

function adopt(raw: RawPermissionCard): PermissionCard {
  return new PermissionCard(
    PERMISSION_CARD_INTERNAL,
    createGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, raw),
  );
}

/**
 * An unforgeable permission card held by this agent.
 *
 * Cards come only from the host: from {@link wallet}, a derive call, or an
 * agent method or RPC result. A card has no public constructor and cannot be
 * serialized. Sending a card to another agent, {@link PermissionCard.revoke},
 * {@link PermissionCard.install} and {@link PermissionCard.release} move it;
 * every later use of a moved card throws.
 *
 * {@link PermissionCard.id}, {@link PermissionCard.parents} and
 * {@link PermissionCard.expiresAt} trap when the host no longer knows the card,
 * for example after it was revoked by another holder.
 */
export class PermissionCard {
  // Makes the class nominal, so a `ScopeCard` is never a `PermissionCard` and
  // the reverse.
  declare private readonly permissionCardBrand: undefined;

  /** @internal */
  constructor(key: PermissionCardInternal, handle: GuestPermissionCardHandle) {
    if (key !== PERMISSION_CARD_INTERNAL) {
      throw new Error('PermissionCard construction is an internal SDK operation');
    }
    if (!(handle instanceof GuestPermissionCardHandle)) {
      throw new Error('PermissionCard can only wrap an opaque permission-card handle');
    }
    bindGuestPermissionCardHandleToCard(key, handle, this);
    cardHandles.set(this, handle);
  }

  /** The card's identifier. */
  id(): CardId {
    return cardIdFromRaw(rawId(borrow(this)));
  }

  /** The identifiers of the card's parents in the derivation DAG. */
  parents(): CardId[] {
    return rawParents(borrow(this)).map(cardIdFromRaw);
  }

  /** The card's expiry, or `undefined` when its lifespan is unbounded. */
  expiresAt(): Timestamp | undefined {
    const raw = rawExpiresAt(borrow(this));
    return raw === undefined
      ? undefined
      : Object.freeze({ seconds: raw.seconds, nanoseconds: raw.nanoseconds });
  }

  /** The card's grant lattice. Requires `card:inspect` authority over the card. */
  inspect(): Result<CardView, PermissionError> {
    const raw = borrow(this);
    return hostCall(() => cardViewFromRaw(inspectCard(raw)));
  }

  /**
   * Derive a narrower child card. The positive lists of `bounds` name the
   * grants of this card to retain and the negative lists name the denials to
   * add. Requires `card:derive` authority over the card.
   */
  derive(bounds: Bounds, lifespan?: Lifespan): Result<PermissionCard, PermissionError> {
    const raw = borrow(this);
    const duration = lifespanToRaw(lifespan);
    return hostCall(() =>
      rawDerive(
        raw,
        grantsToRaw(bounds.lowerPositive),
        grantsToRaw(bounds.lowerNegative),
        grantsToRaw(bounds.upperPositive),
        grantsToRaw(bounds.upperNegative),
        duration,
      ),
    ).map(adopt);
  }

  /**
   * Revoke this card and every card derived from it, returning the number of
   * revoked cards. Moves the card, even when the host reports an error.
   */
  revoke(): Result<number, PermissionError> {
    const raw = take(this);
    return hostCall(() => revokeCard(raw));
  }

  /**
   * Install this card on `target`. Moves the card, even when the host reports
   * an error. Only agent targets are supported.
   */
  install(target: Holder): Result<void, PermissionError> {
    const rawTarget = holderToRaw(target);
    const raw = take(this);
    return hostCall(() => installCard(raw, rawTarget));
  }

  /** Drop this agent's handle to the card without revoking it. Moves the card. */
  release(): void {
    const raw = take(this) as RawPermissionCard & { [Symbol.dispose]?: () => void };
    raw[Symbol.dispose]?.();
  }

  /** `true` after the card was moved; every other operation then throws. */
  isMoved(): boolean {
    return peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handleOf(this)) === undefined;
  }

  toJSON(): never {
    throw new Error(
      'permission cards cannot be serialized; pass them directly to RPC or agent boundaries',
    );
  }
}

/** @internal Borrow the host resource of a card for one host call. */
export function borrowPermissionCardInternal(
  key: PermissionCardInternal,
  card: PermissionCard,
): RawPermissionCard {
  requirePermissionCardInternal(key);
  if (!(card instanceof PermissionCard)) {
    throw new TypeError('expected a PermissionCard');
  }
  return borrow(card);
}

// ============================================================
// Scope card
// ============================================================

interface ScopeCardState {
  raw: RawPermissionCard | undefined;
}

const scopeCardStates = new WeakMap<ScopeCard, ScopeCardState>();

const SCOPE_CARD_MOVED = 'scope card has already been released and can no longer be used';

function scopeStateOf(card: ScopeCard): ScopeCardState {
  const state = scopeCardStates.get(card);
  if (state === undefined) {
    throw new Error('invalid scope card');
  }
  return state;
}

function borrowScope(card: ScopeCard): RawPermissionCard {
  const raw = scopeStateOf(card).raw;
  if (raw === undefined) throw new Error(SCOPE_CARD_MOVED);
  return raw;
}

function borrowParent(parent: PermissionCard | ScopeCard): RawPermissionCard {
  return parent instanceof ScopeCard ? borrowScope(parent) : borrow(parent);
}

/**
 * An invocation-local card from {@link deriveScope}.
 *
 * Attach a scope card to an awaited RPC call (the `scopeCard` call or client
 * option): the callee holds its authority until that invocation ends, and the
 * call borrows the card. A scope card is valid only during the invocation that
 * derived it; keeping it, or a client whose default is it, for a later
 * invocation makes awaited calls fail with `denied`.
 *
 * A scope card cannot be revoked, installed, used as the parent of a
 * persistent card, sent as an RPC argument or result, or serialized. It can be
 * a parent in another {@link deriveScope}.
 */
export class ScopeCard {
  // Makes the class nominal, so a `PermissionCard` is never a `ScopeCard`.
  declare private readonly scopeCardBrand: undefined;

  /** @internal */
  constructor(key: PermissionCardInternal, raw: RawPermissionCard) {
    requirePermissionCardInternal(key);
    scopeCardStates.set(this, { raw });
    registerPermissionCardObject(key, this);
  }

  /** The scope card's identifier. */
  id(): CardId {
    return cardIdFromRaw(rawId(borrowScope(this)));
  }

  /** The identifiers of the scope card's root cards. */
  parents(): CardId[] {
    return rawParents(borrowScope(this)).map(cardIdFromRaw);
  }

  /** The scope card's grant lattice. Requires `card:inspect` authority. */
  inspect(): Result<CardView, PermissionError> {
    const raw = borrowScope(this);
    return hostCall(() => cardViewFromRaw(inspectCard(raw)));
  }

  /** Drop this agent's handle to the scope card. Every later use throws. */
  release(): void {
    const state = scopeStateOf(this);
    const raw = state.raw as (RawPermissionCard & { [Symbol.dispose]?: () => void }) | undefined;
    if (raw === undefined) throw new Error(SCOPE_CARD_MOVED);
    state.raw = undefined;
    raw[Symbol.dispose]?.();
  }

  /** `true` after the scope card was released. */
  isMoved(): boolean {
    return scopeStateOf(this).raw === undefined;
  }

  toJSON(): never {
    throw new Error('scope cards cannot be serialized');
  }
}

/** @internal Borrow the host resource of a scope card for one awaited RPC call. */
export function borrowScopeCardInternal(
  key: PermissionCardInternal,
  card: ScopeCard,
): RawPermissionCard {
  requirePermissionCardInternal(key);
  if (!(card instanceof ScopeCard)) {
    throw new TypeError('expected a ScopeCard from permissions.deriveScope');
  }
  return borrowScope(card);
}

/** @internal Encode a card as a schema value that moves the card when lowered. */
export function permissionCardToSchemaValueInternal(
  key: PermissionCardInternal,
  card: PermissionCard,
): SchemaValue {
  requirePermissionCardInternal(key);
  return v.permissionCard(handleOf(card));
}

/**
 * @internal Decode the handle of a `permission-card` schema value into a card.
 * A handle that another card already holds is rejected, so decoding never
 * moves a card's resource into a second card.
 */
export function permissionCardFromSchemaHandleInternal(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): PermissionCard {
  requirePermissionCardInternal(key);
  if (guestPermissionCardHandleIsBoundToCard(key, handle)) {
    throw new Error('permission-card handle is already held by another card');
  }
  if (permissionCardHandleWasLiftedFromWire(key, handle)) {
    return new PermissionCard(key, handle);
  }
  const raw = releaseGuestPermissionCardHandle(key, handle);
  if (raw === undefined) {
    throw new Error(
      'permission-card handle was already consumed; an owned permission-card can only be decoded once',
    );
  }
  return adopt(raw);
}

/** @internal Wrap a decoded permission-card handle. */
export function permissionCardFromHandleInternal(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): PermissionCard {
  requirePermissionCardInternal(key);
  return new PermissionCard(key, handle);
}

/** @internal Wrap a host-owned permission-card resource. */
export function permissionCardFromRawInternal(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): PermissionCard {
  requirePermissionCardInternal(key);
  return adopt(raw);
}

function requirePermissionCardInternal(key: PermissionCardInternal): void {
  if (key !== PERMISSION_CARD_INTERNAL) {
    throw new Error('this is an internal SDK operation on a permission card');
  }
}

// ============================================================
// Module-level operations
// ============================================================

/**
 * The cards installed in this agent's wallet. The caller owns the returned
 * cards; {@link PermissionCard.release} the ones it does not keep.
 *
 * During an invocation that received a scope card, the host also lists that
 * scope card here. It comes back as a `PermissionCard`, and the host refuses
 * to revoke, install or persist it.
 */
export function wallet(): PermissionCard[] {
  return selfWallet().map(adopt);
}

/** This agent's wallet version. */
export function walletVersion(): WalletVersionToken {
  const raw = selfVersion();
  return Object.freeze({ walletIdHash: raw.walletIdHash, generation: raw.generation });
}

/**
 * Derive a card from this agent's wallet. The lists of `bounds` are the full
 * requested lists.
 */
export function deriveFromWallet(
  bounds: Bounds,
  lifespan?: Lifespan,
): Result<PermissionCard, PermissionError> {
  const duration = lifespanToRaw(lifespan);
  return hostCall(() =>
    rawDeriveFromWallet(
      grantsToRaw(bounds.lowerPositive),
      grantsToRaw(bounds.lowerNegative),
      grantsToRaw(bounds.upperPositive),
      grantsToRaw(bounds.upperNegative),
      duration,
    ),
  ).map(adopt);
}

/**
 * Derive an invocation-local {@link ScopeCard} from one or more persistent or
 * scope parents. The parents are borrowed. The lists of `bounds` are the full
 * requested lists.
 */
export function deriveScope(
  parents: readonly (PermissionCard | ScopeCard)[],
  bounds: Bounds,
): Result<ScopeCard, PermissionError> {
  const rawParents = parents.map(borrowParent);
  return hostCall(() =>
    rawDeriveScope(
      rawParents,
      grantsToRaw(bounds.lowerPositive),
      grantsToRaw(bounds.lowerNegative),
      grantsToRaw(bounds.upperPositive),
      grantsToRaw(bounds.upperNegative),
    ),
  ).map((raw) => new ScopeCard(PERMISSION_CARD_INTERNAL, raw));
}

/** The resource classes and verbs registered in the permission kernel. */
export function listModules(): KernelModule[] {
  return rawListModules().map((module) =>
    Object.freeze({ className: module.className, verbs: Object.freeze([...module.verbs]) }),
  );
}

/** Check that `grant` names a known class, owner, recipient, verb and resource-id pattern. */
export function validateGrant(grant: PatternGrant): Result<void, PermissionError> {
  const raw = grantToRaw(grant);
  return hostCall(() => rawValidateGrant(raw));
}
