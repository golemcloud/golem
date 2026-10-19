/**
 * The permission-card resource's shared data model and inspection operations.
 * The `permission-card` capability itself is defined as a bare resource in
 * `golem:core/types` so that it can travel inside a `schema-value-tree` as an
 * opaque, unforgeable handle. This interface exposes only safe inspection
 * operations over that handle plus the records and variants the other
 * permissions interfaces reference. Components that only need to *hold and
 * pass* cards — most tools and middleware — import only this interface.
 */
declare module 'golem:permissions/types@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  /**
   * --- Card inspection ---
   * Free functions on a borrowed handle, because the resource itself is
   * bare in core (matching secret/quota-token).
   * Stable identifier of this card.
   */
  export function id(c: PermissionCard): CardId;
  /**
   * Parent card identifiers in the derivation DAG.
   */
  export function parents(c: PermissionCard): CardId[];
  /**
   * Expiry timestamp, if the card has a bounded lifespan. For a scope card,
   * the earliest expiry of its root cards.
   */
  export function expiresAt(c: PermissionCard): Timestamp | undefined;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type CardId = golemCore200Types.CardId;
  export type AgentId = golemCore200Types.AgentId;
  export type Uuid = golemCore200Types.Uuid;
  export type Timestamp = {
    seconds: bigint;
    nanoseconds: number;
  };
  /**
   * ISO-8601 duration string (e.g. `"P30D"` for 30 days).
   */
  export type Duration = {
    iso8601: string;
  };
  export type Holder =
  {
    tag: 'account'
    val: Uuid
  } |
  {
    tag: 'app'
    val: Uuid
  } |
  {
    tag: 'agent'
    val: AgentId
  };
  export type PatternGrant = {
    class_: string;
    owner: string;
    recipient: string;
    verb: string;
    resourceId: string;
  };
  export type CardView = {
    lowerPositive: PatternGrant[];
    lowerNegative: PatternGrant[];
    upperPositive: PatternGrant[];
    upperNegative: PatternGrant[];
  };
  export type WalletVersionToken = {
    /** 32 bytes — BLAKE3 of (holder-kind, holder-id). */
    walletIdHash: Uint8Array;
    generation: bigint;
  };
  export type PermissionError =
  {
    tag: 'not-permitted'
    val: string
  } |
  {
    tag: 'lower-bound-too-broad'
    val: string
  } |
  {
    tag: 'upper-bound-too-broad'
    val: string
  } |
  {
    tag: 'wallet-multi-source-required'
    val: string
  } |
  {
    tag: 'card-revoked'
    val: string
  } |
  {
    tag: 'card-expired'
    val: string
  } |
  {
    tag: 'install-recipient-mismatch'
    val: string
  } |
  {
    tag: 'unknown-resource-class'
    val: string
  } |
  {
    tag: 'invalid-pattern'
    val: string
  } |
  {
    tag: 'invalid-verb'
    val: string
  } |
  {
    tag: 'invalid-owner'
    val: string
  } |
  {
    tag: 'invalid-recipient'
    val: string
  };
  /**
   * A persistent derivation result, labelled with its kind.
   */
  export type PersistentCard =
  {
    tag: 'concrete'
    val: PermissionCard
  } |
  {
    tag: 'template'
    val: PermissionCard
  };
  /**
   * A card in the caller's wallet, labelled with its kind.
   */
  export type WalletCard =
  /** A concrete card: every grant names concrete owners and resources. */
  {
    tag: 'concrete'
    val: PermissionCard
  } |
  /**
   * A template card: its grants may contain slot variables (`?agent`,
   * `?env`, ...), which resolve for each holder.
   */
  {
    tag: 'template'
    val: PermissionCard
  } |
  /**
   * The scope card lent to the current invocation. It can be inspected
   * and attached to awaited calls, and is retired at the end of the
   * invocation.
   */
  {
    tag: 'scope'
    val: PermissionCard
  };
}
