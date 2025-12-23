/**
 * Types used by blobstore
 */
declare module 'wasi:blobstore/types' {
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
  export class OutgoingValue {
    static newOutgoingValue(): OutgoingValue;
    outgoingValueWriteBody(): OutputStream;
  }
  export class IncomingValue {
    /**
     * @throws Error
     */
    incomingValueConsumeSync(): IncomingValueSyncBody;
    /**
     * @throws Error
     */
    incomingValueConsumeAsync(): IncomingValueAsyncBody;
    size(): bigint;
  }
  export type InputStream = wasiIo023Streams.InputStream;
  export type OutputStream = wasiIo023Streams.OutputStream;
  /**
   * name of a container, a collection of objects.
   * The container name may be any valid UTF-8 string.
   */
  export type ContainerName = string;
  /**
   * name of an object within a container
   * The object name may be any valid UTF-8 string.
   */
  export type ObjectName = string;
  /**
   * TODO: define timestamp to include seconds since
   * Unix epoch and nanoseconds
   * https://github.com/WebAssembly/wasi-blob-store/issues/7
   */
  export type Timestamp = bigint;
  /**
   * size of an object, in bytes
   */
  export type ObjectSize = bigint;
  export type Error = string;
  /**
   * information about a container
   */
  export type ContainerMetadata = {
    /** the container's name */
    name: ContainerName;
    /** date and time container was created */
    createdAt: Timestamp;
  };
  /**
   * information about an object
   */
  export type ObjectMetadata = {
    /** the object's name */
    name: ObjectName;
    /** the object's parent container */
    container: ContainerName;
    /** date and time the object was created */
    createdAt: Timestamp;
    /** size of the object, in bytes */
    size: ObjectSize;
  };
  /**
   * identifier for an object that includes its container name
   */
  export type ObjectId = {
    container: ContainerName;
    object: ObjectName;
  };
  export type IncomingValueAsyncBody = InputStream;
  export type IncomingValueSyncBody = Uint8Array;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
