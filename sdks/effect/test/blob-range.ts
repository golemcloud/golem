/**
 * The host's ranged-read contract, shared by the blob test doubles so
 * that they cannot drift from each other or from the backends.
 *
 * Both offsets are inclusive, so a range gives `end - start + 1` bytes.
 * A range with a byte that is not in the object is an error. That
 * covers a `start` after the `end`, an `end` at or after the size, and
 * every range of an empty object.
 *
 * The message is the one `BlobRangeError` gives in
 * `golem-service-base/src/storage/blob/mod.rs`, so a test that asserts
 * on it asserts on wording a real backend produces.
 */

export const rangeIsOutsideObject = (start: number, end: number, size: number): boolean =>
  start > end || end >= size

export const rangeErrorMessage = (start: number, end: number): string =>
  `the byte range ${start}-${end} is not in the blob`
