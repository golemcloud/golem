/**
 * A generic keyvalue interface for WASI.
 */
declare module 'wasi:keyvalue/types@0.1.0' {
  import * as wasiKeyvalue010WasiKeyvalueError from 'wasi:keyvalue/wasi-keyvalue-error@0.1.0';
  export class Bucket {
    /**
     * Opens a bucket with the given name.
     * If any error occurs, including if the bucket does not exist, it returns an `Err(error)`.
     * @throws Error
     */
    static openBucket(name: string): Bucket;
  }
  export class OutgoingValue {
    static newOutgoingValue(): OutgoingValue;
    /**
     * Writes the value asynchronously by consuming the given `stream<u8>`:
     * the caller keeps the writable end and the host appends every received
     * byte to the outgoing value's body.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    outgoingValueWriteBodyAsync(data: AsyncIterable<number>): void;
    /**
     * Writes the value synchronously.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    outgoingValueWriteBodySync(value: Uint8Array): void;
  }
  export class IncomingValue {
    /**
     * Consumes the value synchronously and returns the value as a list of bytes.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    incomingValueConsumeSync(): Uint8Array;
    /**
     * Consumes the value asynchronously and returns the value as a `stream<u8>`.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    incomingValueConsumeAsync(): AsyncIterable<number>;
    /**
     * The size of the value in bytes.
     * If the size is unknown or unavailable, this function returns an `Err(error)`.
     * @throws Error
     */
    incomingValueSize(): bigint;
  }
  export type Error = wasiKeyvalue010WasiKeyvalueError.Error;
  /**
   * A key is a unique identifier for a value in a bucket. The key is used to
   * retrieve the value from the bucket.
   */
  export type Key = string;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
