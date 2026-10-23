// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/**
 * A point in time as seconds since the Unix epoch (UTC) plus nanoseconds in
 * `[0, 1_000_000_000)`. Values are plain objects; the `Datetime` helpers
 * convert them to and from `Date` and RFC 3339 text.
 */
export type Datetime = {
  seconds: bigint;
  nanoseconds: number;
};

const SECONDS_PER_DAY = 86_400;
const NANOS_PER_SECOND = 1_000_000_000;
const NANOS_PER_MILLISECOND = 1_000_000;
const MAX_DATE_MILLISECONDS = 8_640_000_000_000_000;
// 0000-01-01T00:00:00Z and 9999-12-31T23:59:59Z: the RFC 3339 four-digit year range.
const MIN_TEXT_SECONDS = -62_167_219_200n;
const MAX_TEXT_SECONDS = 253_402_300_799n;
const RFC3339 =
  /^(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(?:[Zz]|([+-])(\d{2}):(\d{2}))$/u;

function checkDatetime(value: Datetime): void {
  if (typeof value.seconds !== 'bigint') throw new TypeError('Datetime seconds must be a bigint');
  if (
    !Number.isInteger(value.nanoseconds) ||
    value.nanoseconds < 0 ||
    value.nanoseconds >= NANOS_PER_SECOND
  )
    throw new RangeError(`Datetime nanoseconds out of range: ${value.nanoseconds}`);
}

function daysFromCivil(year: number, month: number, day: number): number {
  const y = month <= 2 ? year - 1 : year;
  const era = Math.floor(y / 400);
  const yearOfEra = y - era * 400;
  const dayOfYear = Math.floor((153 * (month + (month > 2 ? -3 : 9)) + 2) / 5) + day - 1;
  const dayOfEra =
    yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
  return era * 146_097 + dayOfEra - 719_468;
}

function civilFromDays(days: number): [number, number, number] {
  const z = days + 719_468;
  const era = Math.floor(z / 146_097);
  const dayOfEra = z - era * 146_097;
  const yearOfEra = Math.floor(
    (dayOfEra -
      Math.floor(dayOfEra / 1_460) +
      Math.floor(dayOfEra / 36_524) -
      Math.floor(dayOfEra / 146_096)) /
      365,
  );
  const dayOfYear =
    dayOfEra - (yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
  const monthIndex = Math.floor((5 * dayOfYear + 2) / 153);
  const day = dayOfYear - Math.floor((153 * monthIndex + 2) / 5) + 1;
  const month = monthIndex < 10 ? monthIndex + 3 : monthIndex - 9;
  return [yearOfEra + era * 400 + (month <= 2 ? 1 : 0), month, day];
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28;
  return [4, 6, 9, 11].includes(month) ? 30 : 31;
}

function pad(value: number, width: number): string {
  return String(value).padStart(width, '0');
}

export const Datetime = {
  /**
   * Parses an RFC 3339 datetime with any UTC offset and converts it to UTC.
   * Throws for malformed text, more than nine fraction digits, and instants
   * outside the years 0000..9999 in UTC.
   */
  parse(value: string): Datetime {
    const match = RFC3339.exec(value);
    if (!match) throw new Error(`Invalid RFC 3339 datetime '${value}'`);
    const [, yearText, monthText, dayText, hourText, minuteText, secondText, fraction, sign] =
      match;
    const [year, month, day, hour, minute, second] = [
      yearText,
      monthText,
      dayText,
      hourText,
      minuteText,
      secondText,
    ].map(Number);
    const offsetHour = sign === undefined ? 0 : Number(match[9]);
    const offsetMinute = sign === undefined ? 0 : Number(match[10]);
    if (
      month < 1 ||
      month > 12 ||
      day < 1 ||
      day > daysInMonth(year, month) ||
      hour > 23 ||
      minute > 59 ||
      second > 59 ||
      offsetHour > 23 ||
      offsetMinute > 59
    )
      throw new Error(`Invalid RFC 3339 datetime '${value}'`);
    if (fraction !== undefined && fraction.length > 9)
      throw new Error(`Datetime '${value}' has more than nine fraction digits`);
    const offset = (sign === '-' ? -1 : 1) * (offsetHour * 3_600 + offsetMinute * 60);
    const seconds =
      BigInt(daysFromCivil(year, month, day)) * BigInt(SECONDS_PER_DAY) +
      BigInt(hour * 3_600 + minute * 60 + second - offset);
    if (seconds < MIN_TEXT_SECONDS || seconds > MAX_TEXT_SECONDS)
      throw new RangeError(`Datetime '${value}' is outside the years 0000..9999 in UTC`);
    return { seconds, nanoseconds: Number((fraction ?? '').padEnd(9, '0')) };
  },

  /**
   * Formats a datetime as RFC 3339 in UTC with a trailing `Z` and up to nine
   * fraction digits (trailing zeros removed). Throws for instants outside the
   * years 0000..9999.
   */
  toISOString(value: Datetime): string {
    checkDatetime(value);
    if (value.seconds < MIN_TEXT_SECONDS || value.seconds > MAX_TEXT_SECONDS)
      throw new RangeError(
        `Datetime seconds ${value.seconds} are outside the years 0000..9999 in UTC`,
      );
    const seconds = Number(value.seconds);
    const days = Math.floor(seconds / SECONDS_PER_DAY);
    const secondOfDay = seconds - days * SECONDS_PER_DAY;
    const [year, month, day] = civilFromDays(days);
    const time = `${pad(Math.floor(secondOfDay / 3_600), 2)}:${pad(Math.floor((secondOfDay % 3_600) / 60), 2)}:${pad(secondOfDay % 60, 2)}`;
    const fraction =
      value.nanoseconds === 0 ? '' : `.${pad(value.nanoseconds, 9).replace(/0+$/u, '')}`;
    return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}T${time}${fraction}Z`;
  },

  /** Converts a `Date` to a datetime. Throws for an invalid `Date`. */
  fromDate(date: Date): Datetime {
    const milliseconds = date.getTime();
    if (!Number.isFinite(milliseconds)) throw new RangeError('Invalid Date');
    const seconds = Math.floor(milliseconds / 1_000);
    return {
      seconds: BigInt(seconds),
      nanoseconds: (milliseconds - seconds * 1_000) * NANOS_PER_MILLISECOND,
    };
  },

  /**
   * Converts a datetime to a `Date`, dropping precision below a millisecond.
   * Throws when the instant is outside the `Date` range.
   */
  toDate(value: Datetime): Date {
    checkDatetime(value);
    const milliseconds =
      Number(value.seconds) * 1_000 + Math.floor(value.nanoseconds / NANOS_PER_MILLISECOND);
    if (!(Math.abs(milliseconds) <= MAX_DATE_MILLISECONDS))
      throw new RangeError(`Datetime seconds ${value.seconds} are outside the Date range`);
    return new Date(milliseconds);
  },
};
