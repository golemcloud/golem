/**
 * Vitest mock for `wasi:blobstore/container`.
 *
 * Backed by an in-memory `Map<containerName, Map<objectName, ObjectEntry>>`
 * shared with the `wasi:blobstore/blobstore` mock. Object names list
 * is paginated by the `StreamObjectNames` resource using the same
 * "pop from tail" semantics as the Golem host.
 */

import {
  IncomingValue,
  type ObjectMetadata,
  OutgoingValue,
  type ContainerMetadata,
} from "./wasi-blobstore-types.js"

interface ObjectEntry {
  bytes: Uint8Array
  createdAt: bigint
}

interface ContainerEntry {
  name: string
  createdAt: bigint
  objects: Map<string, ObjectEntry>
}

const __containers: Map<string, ContainerEntry> = new Map()

export const __ensureContainer = (name: string): ContainerEntry => {
  let entry = __containers.get(name)
  if (!entry) {
    entry = { name, createdAt: BigInt(Date.now()), objects: new Map() }
    __containers.set(name, entry)
  }
  return entry
}

export const __hasContainer = (name: string): boolean => __containers.has(name)
export const __deleteContainerEntry = (name: string): boolean => __containers.delete(name)
export const __getContainerEntry = (name: string): ContainerEntry | undefined =>
  __containers.get(name)

const maybeFail = (op: string, condition: boolean, trace: string): void => {
  if (condition) {
    void op
    throw new Error(trace)
  }
}

export class StreamObjectNames {
  constructor(private _names: string[]) {}
  readStreamObjectNames(len: bigint): [string[], boolean] {
    const out: string[] = []
    const n = Number(len)
    while (out.length < n && this._names.length > 0) {
      // Pop from tail to mirror the Golem host's behaviour.
      const v = this._names.pop()
      if (v !== undefined) out.push(v)
    }
    return [out, this._names.length === 0]
  }
  skipStreamObjectNames(num: bigint): [bigint, boolean] {
    const n = Number(num)
    let skipped = 0
    while (skipped < n && this._names.length > 0) {
      this._names.pop()
      skipped++
    }
    return [BigInt(skipped), this._names.length === 0]
  }
}

export class Container {
  constructor(public readonly _entry: ContainerEntry) {}

  name(): string {
    return this._entry.name
  }

  info(): ContainerMetadata {
    return { name: this._entry.name, createdAt: this._entry.createdAt }
  }

  getData(name: string, start: bigint, end: bigint): IncomingValue {
    const obj = this._entry.objects.get(name)
    maybeFail("getData", obj === undefined, `object ${name} not found`)
    const bytes = obj!.bytes
    const s = Number(start)
    // Mock follows the in-memory backend: `end` is exclusive (Rust slice).
    const e = Math.min(Number(end), bytes.length)
    const slice = bytes.subarray(s, e)
    return new IncomingValue(new Uint8Array(slice))
  }

  writeData(name: string, ov: OutgoingValue): void {
    const bytes = ov.__getBytes()
    this._entry.objects.set(name, { bytes, createdAt: BigInt(Date.now()) })
  }

  listObjects(): StreamObjectNames {
    return new StreamObjectNames(Array.from(this._entry.objects.keys()))
  }

  deleteObject(name: string): void {
    this._entry.objects.delete(name)
  }

  deleteObjects(names: string[]): void {
    for (const n of names) this._entry.objects.delete(n)
  }

  hasObject(name: string): boolean {
    return this._entry.objects.has(name)
  }

  objectInfo(name: string): ObjectMetadata {
    const obj = this._entry.objects.get(name)
    maybeFail("objectInfo", obj === undefined, `object ${name} not found`)
    return {
      name,
      container: this._entry.name,
      createdAt: obj!.createdAt,
      size: BigInt(obj!.bytes.length),
    }
  }

  clear(): void {
    this._entry.objects.clear()
  }
}
