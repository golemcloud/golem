/**
 * a Container is a collection of objects
 */
declare module 'wasi:blobstore/container' {
  import * as wasiBlobstoreTypes from 'wasi:blobstore/types';
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
  export class Container {
    /**
     * returns container name
     * @throws Error
     */
    name(): string;
    /**
     * returns container metadata
     * @throws Error
     */
    info(): ContainerMetadata;
    /**
     * retrieves an object or portion of an object, as a resource.
     * Start and end offsets are inclusive.
     * Once a data-blob resource has been created, the underlying bytes are held by the blobstore service for the lifetime
     * of the data-blob resource, even if the object they came from is later deleted.
     * @throws Error
     */
    getData(name: ObjectName, start: bigint, end: bigint): IncomingValue;
    /**
     * creates or replaces an object with the data blob.
     * @throws Error
     */
    writeData(name: ObjectName, data: OutgoingValue): void;
    /**
     * returns list of objects in the container. Order is undefined.
     * @throws Error
     */
    listObjects(): StreamObjectNames;
    /**
     * deletes object.
     * does not return error if object did not exist.
     * @throws Error
     */
    deleteObject(name: ObjectName): void;
    /**
     * deletes multiple objects in the container
     * @throws Error
     */
    deleteObjects(names: ObjectName[]): void;
    /**
     * returns true if the object exists in this container
     * @throws Error
     */
    hasObject(name: ObjectName): boolean;
    /**
     * returns metadata for the object
     * @throws Error
     */
    objectInfo(name: ObjectName): ObjectMetadata;
    /**
     * removes all objects within the container, leaving the container empty.
     * @throws Error
     */
    clear(): void;
  }
  export class StreamObjectNames {
    /**
     * reads the next number of objects from the stream
     * This function returns the list of objects read, and a boolean indicating if the end of the stream was reached.
     * @throws Error
     */
    readStreamObjectNames(len: bigint): [ObjectName[], boolean];
    /**
     * skip the next number of objects in the stream
     * This function returns the number of objects skipped, and a boolean indicating if the end of the stream was reached.
     * @throws Error
     */
    skipStreamObjectNames(num: bigint): [bigint, boolean];
  }
  export type InputStream = wasiIo023Streams.InputStream;
  export type OutputStream = wasiIo023Streams.OutputStream;
  export type ContainerMetadata = wasiBlobstoreTypes.ContainerMetadata;
  export type Error = wasiBlobstoreTypes.Error;
  export type IncomingValue = wasiBlobstoreTypes.IncomingValue;
  export type ObjectMetadata = wasiBlobstoreTypes.ObjectMetadata;
  export type ObjectName = wasiBlobstoreTypes.ObjectName;
  export type OutgoingValue = wasiBlobstoreTypes.OutgoingValue;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
