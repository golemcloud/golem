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
  export type Error = {
    tag: 'upstream'
    val: string
  } |
  {
    tag: 'io'
    val: string
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
