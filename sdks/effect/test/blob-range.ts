/**
 * The host's ranged-read contract. The blob test doubles share it, so
 * that they answer a ranged read in the same way as each other and as
 * the backends.
 *
 * Both offsets are inclusive. A range gives `end - start + 1` bytes.
 * A range with a byte that is not in the object is an error. This
 * covers a `start` after the `end`. It covers an `end` at or after
 * the size. It covers each range of an empty object.
 *
 * `hostRange` resolves one range. It gives the bounds of the slice,
 * or the message of the error. A double cannot make a slice without
 * the check. A double cannot check an offset that it did not resolve.
 * `hostRange` narrows the bounds to `number` only for a range that is
 * in the object.
 *
 * The offsets are `u64` at the interface, so they stay `bigint` here.
 * `hostRange` wraps each offset to `u64`, because the host does the
 * same. The generated binding takes a `u64` parameter as
 * `BigIntWrapper<u64>` (wasm-rquickjs 0.4.4, `src/types.rs` lines 502
 * to 513). It reads the value with `to_i64`, then casts the result
 * (`skeleton/src/wrappers.rs` lines 199 to 205). `to_i64` calls
 * `JS_ToInt64Ext` (rquickjs-core 0.10.0, `src/value/bigint.rs` lines
 * 23 to 31). For a `BigInt`, that function calls `JS_ToBigInt64Free`,
 * which gives the value mod 2^64 (rquickjs-sys 0.10.0,
 * `quickjs/quickjs.c` line 13517). The host thus reads `-1n` as
 * 18446744073709551615.
 *
 * `BigInt.asUintN(64, x)` gives a value from 0 to 2^64-1. The check
 * therefore never sees a negative offset. It sees the value that the
 * host sees. The tests in `test/blobstore.test.ts` show what the
 * check then answers for a negative offset.
 *
 * The message is the message that a guest gets. `BlobRangeError`
 * gives the text (`golem-service-base/src/storage/blob/mod.rs` lines
 * 466 to 474). The blob store service puts that text in
 * `InvalidInput` (`golem-worker-executor/src/services/blob_store.rs`
 * lines 345 to 346), and `Display` puts the prefix in front of it
 * (lines 41 to 52). The WASI host sends the result to the guest
 * (`golem-worker-executor/src/durable_host/blobstore/container.rs`
 * line 243). `golem-worker-executor/tests/blobstore.rs` line 324
 * asserts the same message. The message says "blob" where this
 * package says "object", because the wording is the host's.
 *
 * The doubles do not follow the host in one point. They look for the
 * object before they look at the range. The host refuses a `start`
 * after the `end` first, and reads nothing
 * (`golem-service-base/src/storage/blob/mod.rs` lines 68 to 70). An
 * inverted range on a missing object is the one case where the two
 * disagree.
 */

/** A range that the host resolved: the bounds of the slice, or the error. */
export type HostRange =
  | { readonly kind: "slice"; readonly first: number; readonly last: number }
  | { readonly kind: "error"; readonly message: string }

export const hostRange = (start: bigint, end: bigint, size: bigint): HostRange => {
  const first = BigInt.asUintN(64, start)
  const last = BigInt.asUintN(64, end)
  return first > last || last >= size
    ? {
        kind: "error",
        message: `Invalid input: the byte range ${first}-${last} is not in the blob`,
      }
    : { kind: "slice", first: Number(first), last: Number(last) }
}
