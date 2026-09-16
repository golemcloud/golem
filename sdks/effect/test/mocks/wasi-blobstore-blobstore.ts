/**
 * Vitest mock for `wasi:blobstore/blobstore`.
 *
 * Implements `createContainer` / `getContainer` / `deleteContainer` /
 * `containerExists` / `copyObject` / `moveObject` over the same
 * in-memory store as the `wasi:blobstore/container` mock.
 */

import {
  Container,
  __ensureContainer,
  __getContainerEntry,
  __hasContainer,
  __deleteContainerEntry,
} from "./wasi-blobstore-container.js"
import type { ObjectId } from "./wasi-blobstore-types.js"

export const createContainer = (name: string): Container => {
  if (__hasContainer(name)) {
    throw new Error(`container ${name} already exists`)
  }
  return new Container(__ensureContainer(name))
}

export const getContainer = (name: string): Container => {
  if (!__hasContainer(name)) {
    throw new Error(`container ${name} does not exist`)
  }
  return new Container(__getContainerEntry(name)!)
}

export const deleteContainer = (name: string): void => {
  __deleteContainerEntry(name)
}

export const containerExists = (name: string): boolean => __hasContainer(name)

export const copyObject = (src: ObjectId, dest: ObjectId): void => {
  const srcEntry = __getContainerEntry(src.container)
  if (!srcEntry) throw new Error(`source container ${src.container} not found`)
  const obj = srcEntry.objects.get(src.object)
  if (!obj) throw new Error(`source object ${src.object} not found`)
  const destEntry = __getContainerEntry(dest.container)
  if (!destEntry) throw new Error(`destination container ${dest.container} not found`)
  destEntry.objects.set(dest.object, { bytes: obj.bytes, createdAt: BigInt(Date.now()) })
}

export const moveObject = (src: ObjectId, dest: ObjectId): void => {
  copyObject(src, dest)
  const srcEntry = __getContainerEntry(src.container)!
  srcEntry.objects.delete(src.object)
}

export type Container_ = Container
