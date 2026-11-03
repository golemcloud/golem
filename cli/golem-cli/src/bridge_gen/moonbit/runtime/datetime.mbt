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

///|
/// A point in time: whole seconds since the Unix epoch plus a nanosecond
/// fraction. The year is in `0000..9999` in UTC, so every value has an RFC 3339
/// form. `Show` and `Datetime::to_string` print it in UTC with a `Z` suffix and
/// up to nine fraction digits.
pub struct Datetime {
  priv seconds : Int64
  priv nanoseconds : UInt
} derive(Eq, Compare, Hash)

///|
let datetime_min_seconds : Int64 = -62167219200L

///|
let datetime_max_seconds : Int64 = 253402300799L

///|
/// Creates a datetime from seconds since the Unix epoch and a nanosecond
/// fraction below one second.
pub fn Datetime::new(
  seconds : Int64,
  nanoseconds : UInt,
) -> Datetime raise BridgeError {
  if nanoseconds >= 1_000_000_000U {
    raise BridgeError(
      "invalid datetime: nanoseconds \{nanoseconds} is not below one second",
    )
  }
  if seconds < datetime_min_seconds || seconds > datetime_max_seconds {
    raise BridgeError(
      "invalid datetime: \{seconds} seconds is outside the years 0000..9999",
    )
  }
  { seconds, nanoseconds }
}

///|
pub fn Datetime::seconds(self : Datetime) -> Int64 {
  self.seconds
}

///|
pub fn Datetime::nanoseconds(self : Datetime) -> UInt {
  self.nanoseconds
}

///|
fn datetime_days_from_civil(year : Int, month : Int, day : Int) -> Int64 {
  let y = (if month <= 2 { year - 1 } else { year }).to_int64()
  let era = if y >= 0L { y / 400L } else { (y - 399L) / 400L }
  let yoe = y - era * 400L
  let m = (if month > 2 { month - 3 } else { month + 9 }).to_int64()
  let doy = (153L * m + 2L) / 5L + day.to_int64() - 1L
  let doe = yoe * 365L + yoe / 4L - yoe / 100L + doy
  era * 146097L + doe - 719468L
}

///|
fn datetime_civil_from_days(days : Int64) -> (Int, Int, Int) {
  let z = days + 719468L
  let era = if z >= 0L { z / 146097L } else { (z - 146096L) / 146097L }
  let doe = z - era * 146097L
  let yoe = (doe - doe / 1460L + doe / 36524L - doe / 146096L) / 365L
  let y = yoe + era * 400L
  let doy = doe - (365L * yoe + yoe / 4L - yoe / 100L)
  let mp = (5L * doy + 2L) / 153L
  let day = doy - (153L * mp + 2L) / 5L + 1L
  let month = if mp < 10L { mp + 3L } else { mp - 9L }
  let year = if month <= 2L { y + 1L } else { y }
  (year.to_int(), month.to_int(), day.to_int())
}

///|
fn datetime_digits(
  value : String,
  start : Int,
  count : Int,
) -> Int raise BridgeError {
  if start + count > value.length() {
    raise BridgeError("invalid RFC 3339 datetime '\{value}'")
  }
  let mut result = 0
  for i in start..<(start + count) {
    let c = value.code_unit_at(i).to_int()
    if c < '0' || c > '9' {
      raise BridgeError("invalid RFC 3339 datetime '\{value}'")
    }
    result = result * 10 + (c - '0')
  }
  result
}

///|
fn datetime_expect(
  value : String,
  index : Int,
  allowed : Array[Int],
) -> Unit raise BridgeError {
  guard index < value.length() &&
    allowed.contains(value.code_unit_at(index).to_int()) else {
    raise BridgeError("invalid RFC 3339 datetime '\{value}'")
  }
}

///|
/// Parses an RFC 3339 datetime such as `2024-01-01T12:30:00.5+02:00`. Any
/// offset is accepted and converted to UTC. The fraction has up to nine
/// digits, and the UTC year must be in `0000..9999`.
pub fn Datetime::parse(value : String) -> Datetime raise BridgeError {
  let year = datetime_digits(value, 0, 4)
  datetime_expect(value, 4, ['-'])
  let month = datetime_digits(value, 5, 2)
  datetime_expect(value, 7, ['-'])
  let day = datetime_digits(value, 8, 2)
  datetime_expect(value, 10, ['T', 't'])
  let hour = datetime_digits(value, 11, 2)
  datetime_expect(value, 13, [':'])
  let minute = datetime_digits(value, 14, 2)
  datetime_expect(value, 16, [':'])
  let second = datetime_digits(value, 17, 2)
  let mut index = 19
  let mut nanoseconds = 0U
  if index < value.length() && value.code_unit_at(index).to_int() == '.' {
    index += 1
    let start = index
    while index < value.length() &&
          value.code_unit_at(index).to_int() >= '0' &&
          value.code_unit_at(index).to_int() <= '9' {
      index += 1
    }
    let digits = index - start
    if digits == 0 || digits > 9 {
      raise BridgeError(
        "invalid RFC 3339 datetime '\{value}': the fraction must have 1 to 9 digits",
      )
    }
    let mut fraction = datetime_digits(value, start, digits).reinterpret_as_uint()
    for _ in digits..<9 {
      fraction = fraction * 10U
    }
    nanoseconds = fraction
  }
  datetime_expect(value, index, ['Z', 'z', '+', '-'])
  let offset_seconds = match value.code_unit_at(index).to_int() {
    'Z' | 'z' => {
      index += 1
      0L
    }
    sign => {
      let offset_hour = datetime_digits(value, index + 1, 2)
      datetime_expect(value, index + 3, [':'])
      let offset_minute = datetime_digits(value, index + 4, 2)
      if offset_hour > 23 || offset_minute > 59 {
        raise BridgeError("invalid RFC 3339 datetime '\{value}': bad offset")
      }
      index += 6
      let offset = (offset_hour * 3600 + offset_minute * 60).to_int64()
      if sign == '-' {
        -offset
      } else {
        offset
      }
    }
  }
  if index != value.length() {
    raise BridgeError("invalid RFC 3339 datetime '\{value}'")
  }
  if month < 1 || month > 12 || hour > 23 || minute > 59 || second > 59 {
    raise BridgeError("invalid RFC 3339 datetime '\{value}'")
  }
  let days = datetime_days_from_civil(year, month, day)
  if day < 1 || datetime_civil_from_days(days) != (year, month, day) {
    raise BridgeError("invalid RFC 3339 datetime '\{value}': no such date")
  }
  let seconds = days * 86400L +
    hour.to_int64() * 3600L +
    minute.to_int64() * 60L +
    second.to_int64() -
    offset_seconds
  if seconds < datetime_min_seconds || seconds > datetime_max_seconds {
    raise BridgeError(
      "invalid RFC 3339 datetime '\{value}': the UTC year is outside 0000..9999",
    )
  }
  { seconds, nanoseconds }
}

///|
fn datetime_write_padded(logger : &Logger, value : Int, width : Int) -> Unit {
  logger.write_string(value.to_string().pad_start(width, '0'))
}

///|
pub impl Show for Datetime with fn output(self, logger) {
  let days = if self.seconds < 0L {
    (self.seconds - 86399L) / 86400L
  } else {
    self.seconds / 86400L
  }
  let second_of_day = (self.seconds - days * 86400L).to_int()
  let (year, month, day) = datetime_civil_from_days(days)
  datetime_write_padded(logger, year, 4)
  logger.write_char('-')
  datetime_write_padded(logger, month, 2)
  logger.write_char('-')
  datetime_write_padded(logger, day, 2)
  logger.write_char('T')
  datetime_write_padded(logger, second_of_day / 3600, 2)
  logger.write_char(':')
  datetime_write_padded(logger, second_of_day % 3600 / 60, 2)
  logger.write_char(':')
  datetime_write_padded(logger, second_of_day % 60, 2)
  if self.nanoseconds != 0U {
    let mut fraction = self.nanoseconds
    let mut digits = 9
    while fraction % 10U == 0U {
      fraction = fraction / 10U
      digits -= 1
    }
    logger.write_char('.')
    datetime_write_padded(logger, fraction.reinterpret_as_int(), digits)
  }
  logger.write_char('Z')
}

///|
pub impl Debug for Datetime with fn to_repr(self) {
  Repr::ctor("Datetime", [(None, Repr::string(self.to_string()))])
}
