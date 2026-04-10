/**
 * A generic keyvalue interface for WASI.
 */
declare module 'wasi:keyvalue/types@0.1.0' {
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
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
     * Writes the value to the output-stream asynchronously.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    outgoingValueWriteBodyAsync(): OutgoingValueBodyAsync;
    /**
     * Writes the value to the output-stream synchronously.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    outgoingValueWriteBodySync(value: OutgoingValueBodySync): void;
  }
  export class IncomingValue {
    /**
     * Consumes the value synchronously and returns the value as a list of bytes.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    incomingValueConsumeSync(): IncomingValueSyncBody;
    /**
     * Consumes the value asynchronously and returns the value as an `input-stream`.
     * If any other error occurs, it returns an `Err(error)`.
     * @throws Error
     */
    incomingValueConsumeAsync(): IncomingValueAsyncBody;
    /**
     * The size of the value in bytes.
     * If the size is unknown or unavailable, this function returns an `Err(error)`.
     * @throws Error
     */
    incomingValueSize(): bigint;
  }
  export type InputStream = wasiIo023Streams.InputStream;
  export type OutputStream = wasiIo023Streams.OutputStream;
  export type Error = wasiKeyvalue010WasiKeyvalueError.Error;
  /**
   * A key is a unique identifier for a value in a bucket. The key is used to
   * retrieve the value from the bucket.
   */
  export type Key = string;
  export type OutgoingValueBodyAsync = OutputStream;
  export type OutgoingValueBodySync = Uint8Array;
  export type IncomingValueAsyncBody = InputStream;
  export type IncomingValueSyncBody = Uint8Array;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
