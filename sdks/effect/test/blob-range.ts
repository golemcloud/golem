/**
 * The host's ranged-read contract, shared by the blob test doubles so
 * that they answer a ranged read the same way as each other and as the
 * backends. One thing they do not share: both doubles look for the
 * object before they look at the range, where the host refuses a
 * `start` after the `end` first, without knowing whether the key
 * exists. An inverted range on a missing object is where the two
 * disagree.
 *
 * Both offsets are inclusive, so a range gives `end - start + 1` bytes.
 * A range with a byte that is not in the object is an error. That
 * covers a `start` after the `end`, an `end` at or after the size, and
 * every range of an empty object.
 *
 * The offsets are `u64` at the interface, so they stay `bigint` here.
 * `hostOffset` gives the value that the host reads for an offset. The
 * generated binding takes a `u64` parameter as `BigIntWrapper<u64>`
 * (wasm-rquickjs 0.4.4, `src/types.rs` lines 502 to 513), which reads
 * the value with `to_i64` and casts the result to `u64`
 * (`skeleton/src/wrappers.rs` lines 199 to 205). `to_i64` truncates a
 * value that is too large, as a note in the same crate says
 * (`skeleton/src/builtin/sqlite.rs` line 263). The host therefore reads
 * `-1n` as 18446744073709551615.
 *
 * Each double wraps both offsets before it looks at the range. The
 * predicate then sees what the host sees, and it needs no case of its
 * own for a negative offset, because a wrapped negative start comes
 * after the end. Without the wrap, a negative offset would reach
 * `subarray`, which counts it from the end of the object, and the
 * double would answer where the host gives an error. A double narrows
 * an offset to `number` only after the predicate has shown that the
 * range lies inside an object the double holds in memory.
 *
 * The message is the one `BlobRangeError` gives in
 * `golem-service-base/src/storage/blob/mod.rs`, so a test that asserts
 * on it asserts on wording a real backend produces. It says "blob"
 * where this package says "object", because the wording is the host's.
 */

export const hostOffset = (offset: bigint): bigint => BigInt.asUintN(64, offset)

export const rangeIsNotInObject = (start: bigint, end: bigint, size: bigint): boolean =>
  start > end || end >= size

export const rangeErrorMessage = (start: bigint, end: bigint): string =>
  `the byte range ${start}-${end} is not in the blob`
