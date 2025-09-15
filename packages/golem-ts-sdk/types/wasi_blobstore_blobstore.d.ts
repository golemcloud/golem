/**
 * wasi-cloud Blobstore service definition
 */
declare module 'wasi:blobstore/blobstore' {
  import * as wasiBlobstoreContainer from 'wasi:blobstore/container';
  import * as wasiBlobstoreTypes from 'wasi:blobstore/types';
  /**
   * creates a new empty container
   */
  export function createContainer(name: ContainerName): Result<Container, Error>;
  /**
   * retrieves a container by name
   */
  export function getContainer(name: ContainerName): Result<Container, Error>;
  /**
   * deletes a container and all objects within it
   */
  export function deleteContainer(name: ContainerName): Result<void, Error>;
  /**
   * returns true if the container exists
   */
  export function containerExists(name: ContainerName): Result<boolean, Error>;
  /**
   * copies (duplicates) an object, to the same or a different container.
   * returns an error if the target container does not exist.
   * overwrites destination object if it already existed.
   */
  export function copyObject(src: ObjectId, dest: ObjectId): Result<void, Error>;
  /**
   * moves or renames an object, to the same or a different container
   * returns an error if the destination container does not exist.
   * overwrites destination object if it already existed.
   */
  export function moveObject(src: ObjectId, dest: ObjectId): Result<void, Error>;
  export type Container = wasiBlobstoreContainer.Container;
  export type Error = wasiBlobstoreTypes.Error;
  export type ContainerName = wasiBlobstoreTypes.ContainerName;
  export type ObjectId = wasiBlobstoreTypes.ObjectId;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
