/**
 * The secret resource, plus the metadata records other interfaces
 * reference. Components that only need to *receive and pass*
 * secrets — most tools, audit middleware, MCP-import bridges —
 * import only this interface.
 */
declare module 'golem:secrets/types@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  /**
   * Stable opaque identifier. Comparing equal across two `secret` handles
   * means "same secret material, same version" — useful for caching and audit
   * correlation. Safe to log, oplog, and emit in traces.
   */
  export function id(s: Secret): SecretId;
  /**
   * Immutable metadata captured at resolve-time. Includes the config-key
   * path, the resolved version, and the resolution timestamp. No plaintext.
   */
  export function metadata(s: Secret): SecretMetadata;
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Secret = golemCore200Types.Secret;
  /**
   * The opaque handle to a sensitive value is defined in
   * `golem:core/types` so it can travel inside a `schema-value-tree` as an
   * unforgeable owned handle. This interface exposes only safe metadata
   * operations over that handle.
   * Resource handles are scoped to a component instance's
   * resource table. When a secret is passed across a component
   * boundary (e.g., agent → tool via `tool-rpc.invoke`'s
   * `value-tree`), the runtime issues a fresh handle in the
   * receiving instance's table that points at the same
   * host-side state. The wire form carries only the
   * `secret-id`, never plaintext.
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
