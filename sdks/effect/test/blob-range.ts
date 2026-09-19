/**
 * The host's ranged-read contract, shared by the blob test doubles so
 * that they answer a ranged read the same way as each other and as the
 * backends. One thing they do not share: both doubles look for the
 * object before they look at the range, where the host refuses a
 * `start` after the `end` first, without knowing whether the key
 * exists. An inverted range on a missing object is the one case where
 * the two disagree.
 *
 * Both offsets are inclusive, so a range gives `end - start + 1` bytes.
 * A range with a byte that is not in the object is an error. That
 * covers a `start` after the `end`, an `end` at or after the size, and
 * every range of an empty object.
 *
 * The offsets are `u64` at the interface, so they stay `bigint` here. A
 * double narrows them to `number` only after this predicate has shown
 * that the range lies inside an object the double holds in memory. A
 * negative offset cannot reach a real host, because the binding rejects
 * it before the call. The predicate refuses it anyway: `subarray` reads
 * a negative index as an offset from the end, so without the check a
 * double would answer with the wrong bytes instead of an error.
 *
 * The message is the one `BlobRangeError` gives in
 * `golem-service-base/src/storage/blob/mod.rs`, so a test that asserts
 * on it asserts on wording a real backend produces. It says "blob"
 * where this package says "object", because the wording is the host's.
 */

export const rangeIsNotInObject = (start: bigint, end: bigint, size: bigint): boolean =>
  start < 0n || start > end || end >= size

export const rangeErrorMessage = (start: bigint, end: bigint): string =>
  `the byte range ${start}-${end} is not in the blob`
