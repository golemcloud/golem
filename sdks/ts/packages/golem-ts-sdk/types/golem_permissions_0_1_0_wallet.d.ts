/**
 * Wallet operations: enumerate the caller's installed cards, query the
 * wallet's version token, and install a card onto a target holder.
 */
declare module 'golem:permissions/wallet@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemPermissions010Types from 'golem:permissions/types@0.1.0';
  /**
   * All permission cards currently installed in the caller's wallet.
   */
  export function selfWallet(): PermissionCard[];
  /**
   * Version token for the caller's wallet, changed whenever the set of
   * installed cards changes.
   */
  export function selfVersion(): WalletVersionToken;
  /**
   * Install `card` onto `target`. Returns unit on success.
   * Only agent targets are currently supported. Account and application
   * targets return `not-permitted("install-card currently supports only
   * agent targets")` before any wallet or oplog mutation.
   * @throws PermissionError
   */
  export function installCard(card: PermissionCard, target: Holder): void;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type PermissionError = golemPermissions010Types.PermissionError;
  export type WalletVersionToken = golemPermissions010Types.WalletVersionToken;
  export type Holder = golemPermissions010Types.Holder;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
