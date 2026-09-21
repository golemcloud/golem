/**
 * The host's ranged-read contract. The blob test doubles share it, so
 * that they give the same result for a ranged read as each other and
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
 * same. The generated binding gives a `u64` parameter the type
 * `BigIntWrapper<u64>` (`get_wrapped_type_internal` in
 * `src/types.rs` of wasm-rquickjs). The `FromJs` implementation of
 * `BigIntWrapper<u64>` reads the value with `BigInt::to_i64`, then
 * casts the result to `u64` (`skeleton/src/wrappers.rs` of
 * wasm-rquickjs). `to_i64` calls `JS_ToInt64Ext` (`src/value/bigint.rs`
 * of rquickjs-core). For a `BigInt`, that function calls
 * `JS_ToBigInt64`, which calls `JS_ToBigInt64Free`. That function
 * gives the value mod 2^64 (`quickjs/quickjs.c` of rquickjs-sys). The
 * host thus reads `-1n` as 18446744073709551615.
 *
 * `BigInt.asUintN(64, x)` gives a value from 0 to 2^64-1. The check
 * therefore never sees a negative offset. It sees the value that the
 * host sees. The tests in `test/blobstore.test.ts` show what the
 * check then gives for a negative offset.
 *
 * The message is the message that a guest gets. `BlobRangeError`
 * gives the text (`golem-service-base/src/storage/blob/mod.rs`).
 * `DefaultBlobStoreService::get_data` puts that text in
 * `BlobStoreError::InvalidInput`, and the `Display` of
 * `BlobStoreError` puts the prefix in front of it
 * (`golem-worker-executor/src/services/blob_store.rs`). The WASI host
 * sends the result to the guest (`HostContainer::get_data` in
 * `golem-worker-executor/src/durable_host/blobstore/container.rs`).
 * The test `blobstore_get_data_outside_the_object_gives_the_guest_an_error`
 * in `golem-worker-executor/tests/blobstore.rs` asserts the same
 * message. The message says "blob" where this package says "object",
 * because the wording is the host's.
 *
 * The doubles do not follow the host in one point. They look for the
 * object before they look at the range. The host rejects a `start`
 * after the `end` first, and reads nothing (`BlobStorage::get_raw_slice`
 * in `golem-service-base/src/storage/blob/mod.rs`). An inverted range
 * on a missing object is the one case where the two disagree.
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
