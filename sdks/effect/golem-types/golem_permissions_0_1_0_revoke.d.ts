/**
 * Revocation of a permission card and its derivation DAG subtree.
 * `revoke-card` consumes the owned handle (the receiving component's
 * resource-table entry is deleted); the stored card is walked via the card
 * store's DAG descendant index. Returns the cut-subtree size.
 */
declare module 'golem:permissions/revoke@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemPermissions010Types from 'golem:permissions/types@0.1.0';
  /**
   * Revoke `c` and every card transitively derived from it.
   * Consumes the owned handle.
   * @throws PermissionError
   */
  export function revokeCard(c: PermissionCard): number;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type PermissionError = golemPermissions010Types.PermissionError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
