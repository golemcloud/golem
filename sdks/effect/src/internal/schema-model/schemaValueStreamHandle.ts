import type { SchemaValueStream, SchemaValueTree } from "golem:core/types@2.0.0"
import { STREAM_INTERNAL, type StreamInternal } from "./streamInternal.js"
import { assertCapabilityReady } from "./capabilityTransaction.js"

export type GuestSchemaValueStream =
  | { kind: "wrapped"; value: SchemaValueStream }
  | { kind: "native"; value: AsyncIterable<SchemaValueTree> }

export interface StreamOwnership {
  available: boolean
  reservation?: object
  busy?: boolean
  iterator?: AsyncIterator<SchemaValueTree>
}

export class GuestSchemaValueStreamHandle {
  #value: GuestSchemaValueStream | undefined
  readonly #ownership: StreamOwnership
  readonly #onTake: (() => void) | undefined
  readonly #onRelease: (() => void) | undefined

  constructor(
    key: StreamInternal,
    value: GuestSchemaValueStream,
    onTake?: () => void,
    onRelease?: () => void,
    ownership: StreamOwnership = { available: true },
  ) {
    if (key !== STREAM_INTERNAL) {
      throw new Error("GuestSchemaValueStreamHandle construction is an internal SDK operation")
    }
    this.#value = value
    this.#ownership = ownership
    this.#onTake = onTake
    this.#onRelease = onRelease
  }

  ownership(key: StreamInternal): StreamOwnership {
    if (key !== STREAM_INTERNAL) throw new Error("stream ownership is internal")
    return this.#ownership
  }

  peek(): GuestSchemaValueStream | undefined {
    return this.#ownership.available ? this.#value : undefined
  }

  reserve(owner: object): void {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== undefined || this.peek() === undefined) {
      throw new Error("schema stream is already reserved or transferred")
    }
    this.#ownership.reservation = owner
  }

  unreserve(owner: object): void {
    if (this.#ownership.reservation === owner) this.#ownership.reservation = undefined
  }

  take(owner?: object): GuestSchemaValueStream | undefined {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== owner)
      throw new Error("schema stream is reserved for transfer")
    this.#ownership.reservation = undefined
    const value = this.peek()
    this.#ownership.available = false
    this.#value = undefined
    if (value !== undefined) this.#onTake?.()
    return value
  }

  release(): void {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== undefined)
      throw new Error("schema stream is reserved for transfer")
    if (this.peek() !== undefined) {
      this.#ownership.available = false
      this.#value = undefined
      this.#onRelease?.()
    }
  }
}
