/**
 * Capability-gated escape hatch: convert a secret resource back
 * to plaintext. The capability is the import — components that
 * do not import this interface cannot reveal secrets. v1 grants
 * reveal at the import-or-not level only; v2 introduces
 * per-(agent, tool, tool-middleware) manifest binding axes that narrow
 * reveal even when imported (§5.6).
 * Every successful reveal is recorded in the calling agent's
 * oplog as `(calling-agent, secret-id, timestamp)`. The
 * plaintext bytes are not part of the audit record.
 * Tools that consume secrets at the wire boundary (HTTP
 * authorization headers, signing operations, encryption) SHOULD
 * prefer host-mediated substitution over reveal — host
 * capabilities accepting `borrow<secret>` directly let the
 * runtime substitute plaintext at the syscall boundary, never
 * crossing into guest linear memory at all. Reveal is the
 * fallback for genuinely custom protocols the host doesn't
 * natively support; its use is loud by design.
 */
declare module 'golem:secrets/reveal@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemSecrets010Types from 'golem:secrets/types@0.1.0';
  /**
   * Unpack a secret resource to its inner typed value. `expected` is the
   * guest's inner-type graph; the host validates it against the secret's
   * pinned inner type and returns the stored value as a schema-value-tree.
   * @throws SecretError
   */
  export function reveal(s: Secret, expected: SchemaGraph): SchemaValueTree;
  export type SecretError = golemSecrets010Types.SecretError;
  export type Secret = golemCore200Types.Secret;
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type SchemaValueTree = golemCore200Types.SchemaValueTree;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
