/**
 * Canonical schema and lossless conversions for
 * `wasi:clocks/types@0.3.0`.Datetime.
 *
 * JavaScript `Date`, Effect `DateTime`, and numeric epoch timestamps have
 * millisecond precision. Converting a WIT datetime to one of those forms
 * therefore rejects sub-millisecond values rather than truncating them.
 *
 * @since 1.5.1
 */
import { DateTime as EffectDateTime, Effect, Schema } from "effect"
import { Uint32, Uint64, witTypeAnnotationKey } from "./WitTypes.js"

const U64_MAX = 2n ** 64n - 1n
const NANOS_PER_SECOND = 1_000_000_000
const NANOS_PER_MILLISECOND = 1_000_000
const MILLIS_PER_SECOND = 1_000n
const MAX_DATE_EPOCH_MILLISECONDS = 8_640_000_000_000_000

const Seconds = Uint64.check(Schema.isBetweenBigInt({ minimum: 0n, maximum: U64_MAX })).annotate({
  [witTypeAnnotationKey]: "u64",
})
const Nanoseconds = Uint32.check(
  Schema.isInt(),
  Schema.isBetween({ minimum: 0, maximum: NANOS_PER_SECOND - 1 }),
).annotate({ [witTypeAnnotationKey]: "u32" })

/**
 * Schema for `wasi:clocks/types@0.3.0`.Datetime.
 *
 * The seconds field is an unsigned 64-bit integer and nanoseconds must be in
 * the range 0 through 999,999,999, matching the WIT clock contract.
 *
 * @since 1.5.1
 * @category codecs
 */
export const Datetime = Schema.Struct({
  seconds: Seconds,
  nanoseconds: Nanoseconds,
})

/**
 * A validated `wasi:clocks/types@0.3.0`.Datetime value.
 *
 * @since 1.5.1
 * @category models
 */
export type Datetime = typeof Datetime.Type

/**
 * Input accepted by {@link fromInput}. Numbers are interpreted as epoch
 * milliseconds.
 *
 * @since 1.5.1
 * @category models
 */
export type Input = Datetime | Date | EffectDateTime.DateTime | number

/**
 * A datetime could not be represented without losing precision or exceeding
 * the range supported by the target representation.
 *
 * @since 1.5.1
 * @category errors
 */
export class DatetimeConversionError {
  readonly _tag = "DatetimeConversionError"

  constructor(readonly message: string) {}
}

const conversionError = (message: string): DatetimeConversionError =>
  new DatetimeConversionError(message)

const validate = (value: unknown): Effect.Effect<Datetime, DatetimeConversionError> =>
  Schema.decodeUnknownEffect(Datetime)(value).pipe(
    Effect.mapError((cause) => conversionError(`Invalid WIT datetime: ${String(cause)}`)),
  )

/**
 * Convert non-negative epoch milliseconds to a WIT datetime.
 *
 * The input must be an integer within JavaScript's valid `Date` range. Negative
 * timestamps are rejected because the WIT seconds field is unsigned.
 *
 * @since 1.5.1
 * @category conversions
 */
export const fromEpochMilliseconds = (
  epochMilliseconds: number,
): Effect.Effect<Datetime, DatetimeConversionError> => {
  if (!Number.isSafeInteger(epochMilliseconds)) {
    return Effect.fail(conversionError("Epoch milliseconds must be a finite safe integer"))
  }
  if (epochMilliseconds < 0) {
    return Effect.fail(
      conversionError("WIT datetime cannot represent an instant before the Unix epoch"),
    )
  }
  if (epochMilliseconds > MAX_DATE_EPOCH_MILLISECONDS) {
    return Effect.fail(conversionError("Epoch milliseconds exceed the JavaScript Date range"))
  }

  const seconds = Math.floor(epochMilliseconds / Number(MILLIS_PER_SECOND))
  const milliseconds = epochMilliseconds % Number(MILLIS_PER_SECOND)
  return Effect.succeed({
    seconds: BigInt(seconds),
    nanoseconds: milliseconds * NANOS_PER_MILLISECOND,
  })
}

/**
 * Convert a valid JavaScript `Date` to a WIT datetime.
 *
 * @since 1.5.1
 * @category conversions
 */
export const fromDate = (date: Date): Effect.Effect<Datetime, DatetimeConversionError> => {
  const epochMilliseconds = date.getTime()
  if (Number.isNaN(epochMilliseconds)) {
    return Effect.fail(conversionError("Cannot convert an invalid JavaScript Date"))
  }
  return fromEpochMilliseconds(epochMilliseconds)
}

/**
 * Convert an Effect `DateTime` to a WIT datetime. Zoned values are converted by
 * their absolute instant; their display timezone does not change the result.
 *
 * @since 1.5.1
 * @category conversions
 */
export const fromDateTime = (
  dateTime: EffectDateTime.DateTime,
): Effect.Effect<Datetime, DatetimeConversionError> =>
  fromEpochMilliseconds(dateTime.epochMilliseconds)

/**
 * Normalize a WIT datetime, JavaScript `Date`, Effect `DateTime`, or numeric
 * epoch-millisecond value to a validated WIT datetime.
 *
 * @since 1.5.1
 * @category conversions
 */
export const fromInput = (input: Input): Effect.Effect<Datetime, DatetimeConversionError> => {
  if (typeof input === "number") return fromEpochMilliseconds(input)
  if (input instanceof Date) return fromDate(input)
  if (EffectDateTime.isDateTime(input)) return fromDateTime(input)
  return validate(input)
}

/**
 * Convert a WIT datetime to epoch milliseconds without losing precision.
 *
 * Values with sub-millisecond nanoseconds or outside JavaScript's valid `Date`
 * range are rejected.
 *
 * @since 1.5.1
 * @category conversions
 */
export const toEpochMilliseconds = (
  datetime: Datetime,
): Effect.Effect<number, DatetimeConversionError> =>
  Effect.flatMap(validate(datetime), ({ seconds, nanoseconds }) => {
    if (nanoseconds % NANOS_PER_MILLISECOND !== 0) {
      return Effect.fail(
        conversionError("Cannot convert sub-millisecond nanoseconds without losing precision"),
      )
    }

    const epochMilliseconds =
      seconds * MILLIS_PER_SECOND + BigInt(nanoseconds / NANOS_PER_MILLISECOND)
    if (epochMilliseconds > BigInt(MAX_DATE_EPOCH_MILLISECONDS)) {
      return Effect.fail(conversionError("WIT datetime exceeds the JavaScript Date range"))
    }

    return Effect.succeed(Number(epochMilliseconds))
  })

/**
 * Convert a WIT datetime to a JavaScript `Date` without losing precision.
 *
 * @since 1.5.1
 * @category conversions
 */
export const toDate = (datetime: Datetime): Effect.Effect<Date, DatetimeConversionError> =>
  Effect.map(toEpochMilliseconds(datetime), (epochMilliseconds) => new Date(epochMilliseconds))

/**
 * Convert a WIT datetime to an Effect UTC `DateTime` without losing precision.
 *
 * @since 1.5.1
 * @category conversions
 */
export const toDateTime = (
  datetime: Datetime,
): Effect.Effect<EffectDateTime.Utc, DatetimeConversionError> =>
  Effect.map(toDate(datetime), EffectDateTime.fromDateUnsafe)
