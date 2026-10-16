/**
 * Read-only inspection of a permission card's full grant lattice.
 * Returns the four quadrants of the card's bound lattice
 * (lower/upper × positive/negative) as a `card-view`. Importing this
 * interface grants no authority to mutate or derive cards.
 */
declare module 'golem:permissions/inspect@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemPermissions010Types from 'golem:permissions/types@0.1.0';
  /**
   * Return the card's complete grant lattice.
   * @throws PermissionError
   */
  export function inspectCard(c: PermissionCard): CardView;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type CardView = golemPermissions010Types.CardView;
  export type PermissionError = golemPermissions010Types.PermissionError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
