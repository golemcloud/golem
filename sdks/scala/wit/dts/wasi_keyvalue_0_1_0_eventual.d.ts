/**
 * A keyvalue interface that provides eventually consistent CRUD operations.
 * A CRUD operation is an operation that acts on a single key-value pair.
 * The value in the key-value pair is defined as a `u8` byte array and the intention
 * is that it is the common denominator for all data types defined by different
 * key-value stores to handle data, ensuring compatibility between different
 * key-value stores. Note: the clients will be expecting serialization/deserialization overhead
 * to be handled by the key-value store. The value could be a serialized object from
 * JSON, HTML or vendor-specific data types like AWS S3 objects.
 * Data consistency in a key value store refers to the gaurantee that once a
 * write operation completes, all subsequent read operations will return the
 * value that was written.
 * The level of consistency in readwrite interfaces is **eventual consistency**,
 * which means that if a write operation completes successfully, all subsequent
 * read operations will eventually return the value that was written. In other words,
 * if we pause the updates to the system, the system eventually will return
 * the last updated value for read.
 */
declare module 'wasi:keyvalue/eventual@0.1.0' {
  import * as wasiKeyvalue010Types from 'wasi:keyvalue/types@0.1.0';
  /**
   * Get the value associated with the key in the bucket.
   * The value is returned as an option. If the key-value pair exists in the
   * bucket, it returns `Ok(value)`. If the key does not exist in the
   * bucket, it returns `Ok(none)`.
   * If any other error occurs, it returns an `Err(error)`.
   * @throws Error
   */
  export function get(bucket: Bucket, key: Key): IncomingValue | undefined;
  /**
   * Set the value associated with the key in the bucket. If the key already
   * exists in the bucket, it overwrites the value.
   * If the key does not exist in the bucket, it creates a new key-value pair.
   * If any other error occurs, it returns an `Err(error)`.
   * @throws Error
   */
  export function set(bucket: Bucket, key: Key, outgoingValue: OutgoingValue): void;
  /**
   * Delete the key-value pair associated with the key in the bucket.
   * If the key does not exist in the bucket, it does nothing.
   * If any other error occurs, it returns an `Err(error)`.
   * @throws Error
   */
  export function delete_(bucket: Bucket, key: Key): void;
  /**
   * Check if the key exists in the bucket.
   * If the key exists in the bucket, it returns `Ok(true)`. If the key does
   * not exist in the bucket, it returns `Ok(false)`.
   * If any other error occurs, it returns an `Err(error)`.
   * @throws Error
   */
  export function exists(bucket: Bucket, key: Key): boolean;
  export type Bucket = wasiKeyvalue010Types.Bucket;
  export type Error = wasiKeyvalue010Types.Error;
  export type IncomingValue = wasiKeyvalue010Types.IncomingValue;
  export type Key = wasiKeyvalue010Types.Key;
  export type OutgoingValue = wasiKeyvalue010Types.OutgoingValue;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
