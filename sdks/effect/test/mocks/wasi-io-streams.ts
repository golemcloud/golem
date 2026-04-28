/**
 * Vitest mock for `wasi:io/streams@0.2.3`.
 *
 * Only the bits used by the blobstore wrapper are exposed:
 * - `OutputStream.blockingWriteAndFlush(bytes)` — appends to an
 *   internal `Uint8Array[]` buffer (cumulative).
 * - `InputStream` — supports `blockingRead(len)` returning consecutive
 *   chunks, plus `read(len)`.
 *
 * Mock state is per-instance; tests construct streams with
 * `MockOutputStream.create()` / `MockInputStream.fromBytes(buf)`.
 */

export type StreamError =
  | { tag: "last-operation-failed"; val: { message: string } }
  | { tag: "closed" }

export class OutputStream {
  /** @internal */
  private _buf: number[] = []
  /** @internal */
  _closed = false

  static create(): OutputStream {
    return new OutputStream()
  }

  /** Cumulative captured bytes (test-only accessor). */
  __getBytes(): Uint8Array {
    return new Uint8Array(this._buf)
  }

  __close(): void {
    this._closed = true
  }

  checkWrite(): bigint {
    if (this._closed) {
      throw { tag: "closed" }
    }
    return 4096n
  }

  write(bytes: Uint8Array): void {
    if (this._closed) throw { tag: "closed" }
    for (const b of bytes) this._buf.push(b)
  }

  blockingWriteAndFlush(bytes: Uint8Array): void {
    if (this._closed) throw { tag: "closed" }
    if (bytes.length > 4096) {
      throw new Error("blockingWriteAndFlush requires len <= 4096")
    }
    for (const b of bytes) this._buf.push(b)
  }

  flush(): void {}
  blockingFlush(): void {}
}

export class InputStream {
  private _idx = 0
  constructor(private readonly _bytes: Uint8Array) {}

  static fromBytes(b: Uint8Array): InputStream {
    return new InputStream(b)
  }

  read(len: bigint): Uint8Array {
    return this.blockingRead(len)
  }

  blockingRead(len: bigint): Uint8Array {
    if (this._idx >= this._bytes.length) {
      throw { tag: "closed" }
    }
    const n = Number(len)
    const end = Math.min(this._idx + n, this._bytes.length)
    const slice = this._bytes.subarray(this._idx, end)
    this._idx = end
    return new Uint8Array(slice)
  }

  skip(len: bigint): bigint {
    return this.blockingSkip(len)
  }

  blockingSkip(len: bigint): bigint {
    const n = Number(len)
    const start = this._idx
    this._idx = Math.min(this._idx + n, this._bytes.length)
    return BigInt(this._idx - start)
  }
}
