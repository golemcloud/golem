/**
 * wasi-cloud Blobstore service definition
 */
declare module 'wasi:blobstore/blobstore' {
  import * as wasiBlobstoreContainer from 'wasi:blobstore/container';
  import * as wasiBlobstoreTypes from 'wasi:blobstore/types';
  /**
   * creates a new empty container
   * @throws Error
   */
  export function createContainer(name: ContainerName): Container;
  /**
   * retrieves a container by name
   * @throws Error
   */
  export function getContainer(name: ContainerName): Container;
  /**
   * deletes a container and all objects within it
   * @throws Error
   */
  export function deleteContainer(name: ContainerName): void;
  /**
   * returns true if the container exists
   * @throws Error
   */
  export function containerExists(name: ContainerName): boolean;
  /**
   * copies (duplicates) an object, to the same or a different container.
   * returns an error if the target container does not exist.
   * overwrites destination object if it already existed.
   * @throws Error
   */
  export function copyObject(src: ObjectId, dest: ObjectId): void;
  /**
   * moves or renames an object, to the same or a different container
   * returns an error if the destination container does not exist.
   * overwrites destination object if it already existed.
   * @throws Error
   */
  export function moveObject(src: ObjectId, dest: ObjectId): void;
  export type Container = wasiBlobstoreContainer.Container;
  export type Error = wasiBlobstoreTypes.Error;
  export type ContainerName = wasiBlobstoreTypes.ContainerName;
  export type ObjectId = wasiBlobstoreTypes.ObjectId;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
