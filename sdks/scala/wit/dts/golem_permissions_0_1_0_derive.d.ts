/**
 * Derivation of new permission cards from an existing card or from the
 * caller's wallet.
 * Derivation narrows authority: a child card's lattice is bounded by its
 * parent's lattice. The caller specifies which lower-positive and
 * upper-positive grants to retain and which lower-negative and
 * upper-negative grants to add, plus an optional lifespan.
 * Persistent derivation returns a template if any requested grant contains
 * a slot variable (`?agent`, `?env`, ...), otherwise a concrete card. Slots
 * name the eventual holder, not the caller. Every possible instantiation
 * must be bounded by the parent. A template parent in the caller's wallet
 * is resolved for the caller before validation.
 */
declare module 'golem:permissions/derive@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemPermissions010Types from 'golem:permissions/types@0.1.0';
  /**
   * Derive a child card from `parent`, narrowing the grant lattice.
   * A template parent (one whose grants carry slot variables) must be in
   * the caller's wallet and is resolved against the caller's identity.
   * Only its grants addressed to the caller can be retained.
   * @throws PermissionError
   */
  export function derive(parent: PermissionCard, lowerPositiveToRetain: PatternGrant[], lowerNegativeToAdd: PatternGrant[], upperPositiveToRetain: PatternGrant[], upperNegativeToAdd: PatternGrant[], lifespan: Duration | undefined): PersistentCard;
  /**
   * Derive a card directly from the caller's wallet.
   * @throws PermissionError
   */
  export function deriveFromWallet(lowerPositive: PatternGrant[], lowerNegative: PatternGrant[], upperPositive: PatternGrant[], upperNegative: PatternGrant[], lifespan: Duration | undefined): PersistentCard;
  /**
   * Derive an invocation-local card from one or more persistent or scope
   * parents. Scope cards are attenuated in-memory and are never persisted.
   * Requested grants must be concrete; slots return `invalid-pattern`.
   * @throws PermissionError
   */
  export function deriveScope(parents: PermissionCard[], lowerPositive: PatternGrant[], lowerNegative: PatternGrant[], upperPositive: PatternGrant[], upperNegative: PatternGrant[]): PermissionCard;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type PatternGrant = golemPermissions010Types.PatternGrant;
  export type PermissionError = golemPermissions010Types.PermissionError;
  export type Duration = golemPermissions010Types.Duration;
  export type PersistentCard = golemPermissions010Types.PersistentCard;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
