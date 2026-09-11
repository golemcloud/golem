declare module 'wasi:config/store@0.2.0-draft' {
  /**
   * Gets a configuration value of type `string` associated with the `key`.
   * The value is returned as an `option<string>`. If the key is not found,
   * `Ok(none)` is returned. If an error occurs, an `Err(error)` is returned.
   * @throws Error
   */
  export function get(key: string): string | undefined;
  /**
   * Gets a list of configuration key-value pairs of type `string`.
   * If an error occurs, an `Err(error)` is returned.
   * @throws Error
   */
  export function getAll(): [string, string][];
  /**
   * An error type that encapsulates the different errors that can occur fetching configuration values.
   */
  export type Error = 
  /**
   * This indicates an error from an "upstream" config source.
   * As this could be almost _anything_ (such as Vault, Kubernetes ConfigMaps, KeyValue buckets, etc),
   * the error message is a string.
   */
  {
    tag: 'upstream'
    val: string
  } |
  /**
   * This indicates an error from an I/O operation.
   * As this could be almost _anything_ (such as a file read, network connection, etc),
   * the error message is a string.
   * Depending on how this ends up being consumed,
   * we may consider moving this to use the `wasi:io/error` type instead.
   * For simplicity right now in supporting multiple implementations, it is being left as a string.
   */
  {
    tag: 'io'
    val: string
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
