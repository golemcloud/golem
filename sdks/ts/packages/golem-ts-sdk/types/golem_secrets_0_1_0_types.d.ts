/**
 * The secret resource, plus the metadata records other interfaces
 * reference. Components that only need to *receive and pass*
 * secrets — most tools, audit middleware, MCP-import bridges —
 * import only this interface.
 */
declare module 'golem:secrets/types@0.1.0' {
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  export class Secret {
    /**
     * Stable opaque identifier. Comparing equal across two
     * `secret` handles means "same secret material, same
     * version" — useful for caching and audit correlation.
     * Safe to log, oplog, and emit in traces.
     */
    id(): SecretId;
    /**
     * Immutable metadata captured at resolve-time. Includes the
     * config-key path, the resolved version, and the resolution
     * timestamp. No plaintext.
     */
    metadata(): SecretMetadata;
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  /**
   * Stable, opaque identifier scoped to the deployment.
   */
  export type SecretId = {
    bytes: Uint8Array;
  };
  /**
   * A secret-store-assigned, monotonic version identifier. The
   * secret-store backend defines the ordering and the format;
   * the type-tree treats it as opaque.
   */
  export type SecretVersion = {
    bytes: Uint8Array;
  };
  export type SecretMetadata = {
    /**
     * Config-key path the secret resolved from, when applicable.
     * `none` for secrets minted by `golem:secrets/create` or
     * returned in tool-result position.
     */
    configKey?: string[];
    /**
     * Pinned version captured at resolve-time. `none` for
     * dynamic-origin secrets (created from a string or returned
     * from a tool that didn't itself have a versioned source).
     */
    version?: SecretVersion;
    /** Time of resolution. */
    resolvedAt: Datetime;
    /**
     * The semantic category declared on the secret's schema type, when
     * known. Mirrors `secret-spec.category` in `golem:core/types`.
     */
    category?: string;
  };
  /**
   * Errors common to operations on secrets.
   */
  export type SecretError = 
  /**
   * The secret was bound but its current resolution failed
   * (e.g., the secret-store entry was deleted between binding
   * and reveal, or the network to the store is partitioned).
   */
  {
    tag: 'unavailable'
    val: string
  } |
  /**
   * The secret was version-pinned and the pinned version no
   * longer exists (administratively destroyed).
   */
  {
    tag: 'version-not-found'
    val: SecretVersion
  } |
  /**
   * Internal runtime error (carries an opaque message; never
   * includes plaintext).
   */
  {
    tag: 'internal'
    val: string
  };
}
