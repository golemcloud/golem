/**
 * WASI Monotonic Clock is a clock API intended to let users measure elapsed
 * time.
 * It is intended to be portable at least between Unix-family platforms and
 * Windows.
 * A monotonic clock is a clock which has an unspecified initial value, and
 * successive reads of the clock will produce non-decreasing values.
 */
declare module 'wasi:clocks/monotonic-clock@0.3.0' {
  import * as wasiClocks030Types from 'wasi:clocks/types@0.3.0';
  /**
   * Read the current value of the clock.
   * The clock is monotonic, therefore calling this function repeatedly will
   * produce a sequence of non-decreasing values.
   * For completeness, this function traps if it's not possible to represent
   * the value of the clock in a `mark`. Consequently, implementations
   * should ensure that the starting time is low enough to avoid the
   * possibility of overflow in practice.
   */
  export function now(): Mark;
  /**
   * Query the resolution of the clock. Returns the duration of time
   * corresponding to a clock tick.
   */
  export function getResolution(): Duration;
  /**
   * Wait until the specified mark has occurred.
   */
  export function waitUntil(when: Mark): Promise<void>;
  /**
   * Wait for the specified duration to elapse.
   */
  export function waitFor(howLong: Duration): Promise<void>;
  export type Duration = wasiClocks030Types.Duration;
  /**
   * A mark on a monotonic clock is a number of nanoseconds since an
   * unspecified initial value, and can only be compared to instances from
   * the same monotonic-clock.
   */
  export type Mark = bigint;
}
