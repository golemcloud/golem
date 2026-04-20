// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// An amount of time with nanosecond precision.
struct Duration {
  secs : Int64
  nanos : Int
} derive(Eq, Compare)

///|
/// Creates a Duration from hours, minutes, seconds and nanoseconds.
pub fn Duration::of(
  hours? : Int64 = 0L,
  minutes? : Int64 = 0L,
  seconds? : Int64 = 0L,
  nanoseconds? : Int64 = 0L,
) -> Duration raise Error {
  let hours_sec = checked_mul_int64(hours, seconds_per_hour)
  let minutes_sec = checked_mul_int64(minutes, seconds_per_minute)
  let mut secs = checked_add_int64(
    checked_add_int64(hours_sec, minutes_sec),
    seconds,
  )
  let nanos = (nanoseconds % nanoseconds_per_second).to_int()
  secs += nanoseconds / nanoseconds_per_second
  { secs, nanos }
}

///|
fn create(secs : Int64, nanos : Int) -> Duration raise Error {
  if nanos < min_nanosecond || nanos > max_nanosecond {
    fail(invalid_duration_err)
  }
  { secs, nanos }
}

///|
/// Returns a zero length duration.
pub fn Duration::zero() -> Duration {
  { secs: 0L, nanos: 0 }
}

///|
/// Parses a ISO 8601 format string like `PT[n]H[n]M[n].[n]S`.
pub fn Duration::from_string(str : String) -> Duration raise Error {
  guard str is [.. "PT", ..] else { fail(invalid_duration_err) }
  let mut hours = 0L
  let mut minutes = 0L
  let mut seconds = 0L
  let mut nanoseconds = 0L
  let buf = StringBuilder::new(size_hint=0)
  for i = 2; i < str.length(); i = i + 1 {
    match str.code_unit_at(i) {
      '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | '.' | '-' =>
        buf.write_char(str.code_unit_at(i).unsafe_to_char())
      'H' => {
        hours = @strconv.from_str(buf.to_string())
        buf.reset()
      }
      'M' => {
        minutes = @strconv.from_str(buf.to_string())
        buf.reset()
      }
      'S' => {
        let s = buf.to_string()
        let splits = split(s, '.')
        if splits.is_empty() || splits.length() > 2 {
          fail(invalid_duration_err)
        }
        if splits.length() > 0 && splits[0] != "" {
          let secs_str = splits[0]
          seconds = @strconv.from_str(secs_str)
        }
        if splits.length() > 1 && splits[1] != "" {
          let mut nanos_str = splits[1]
          nanos_str = add_suffix_zero(nanos_str, 9)
          if seconds < 0L {
            nanos_str = "-" + nanos_str
          }
          nanoseconds = @strconv.from_str(nanos_str)
        }
        break
      }
      _ => fail(invalid_duration_err)
    }
  }
  Duration::of(hours~, minutes~, seconds~, nanoseconds~)
}

///|
/// Returns a string representation of this duration using ISO 8601 representation.
pub fn Duration::to_string(self : Duration) -> String {
  if self.is_zero() {
    return "PT0S"
  }
  let buf = StringBuilder::new(size_hint=0)
  buf.write_string("PT")
  let total_secs = self.secs
  let hours = total_secs / seconds_per_hour
  let minutes = total_secs % seconds_per_hour / seconds_per_minute
  let seconds = total_secs % seconds_per_minute
  let nanos_of_seconds = seconds * nanoseconds_per_second +
    self.nanos.to_int64()
  let secs = (nanos_of_seconds / nanoseconds_per_second).abs()
  let nanos = (nanos_of_seconds % nanoseconds_per_second).abs()
  if hours != 0L {
    buf.write_string(hours.to_string())
    buf.write_char('H')
  }
  if minutes != 0L {
    buf.write_string(minutes.to_string())
    buf.write_char('M')
  }
  if secs != 0L || nanos > 0L {
    if nanos_of_seconds < 0L {
      buf.write_char('-')
    }
    buf.write_string(secs.to_string())
    if nanos > 0L {
      buf.write_string(format_nanos(nanos).to_string())
    }
    buf.write_char('S')
  }
  buf.to_string()
}

///|
pub impl Show for Duration with output(self : Duration, logger : &Logger) -> Unit {
  logger.write_string(self.to_string())
}

///|
/// Returns the number of seconds in this duration.
pub fn Duration::seconds(self : Duration) -> Int64 {
  self.secs
}

///|
/// Returns the number of nanoseconds in this duration.
pub fn Duration::nanoseconds(self : Duration) -> Int {
  self.nanos
}

///|
/// Checks if this duration is zero length.
pub fn Duration::is_zero(self : Duration) -> Bool {
  self.secs == 0L && self.nanos == 0
}

///|
/// Checks if this duration is negative.
pub fn Duration::is_neg(self : Duration) -> Bool {
  self.secs < 0L
}

///|
/// Adds specified hours to this duration, and returns a new duration.
pub fn Duration::add_hours(
  self : Duration,
  hours : Int64,
) -> Duration raise Error {
  if hours == 0L {
    return self
  }
  let secs = checked_add_int64(
    self.secs,
    checked_mul_int64(hours, seconds_per_hour),
  )
  { ..self, secs, }
}

///|
/// Adds specified minutes to this duration, and returns a new duration.
pub fn Duration::add_minutes(
  self : Duration,
  minutes : Int64,
) -> Duration raise Error {
  if minutes == 0L {
    return self
  }
  let secs = checked_add_int64(
    self.secs,
    checked_mul_int64(minutes, seconds_per_minute),
  )
  { ..self, secs, }
}

///|
/// Adds specified seconds to this duration, and returns a new duration.
pub fn Duration::add_seconds(
  self : Duration,
  seconds : Int64,
) -> Duration raise Error {
  if seconds == 0L {
    return self
  }
  let secs = checked_add_int64(self.secs, seconds)
  { ..self, secs, }
}

///|
/// Adds specified nanoseconds to this duration, and returns a new duration.
pub fn Duration::add_nanoseconds(
  self : Duration,
  nanoseconds : Int64,
) -> Duration raise Error {
  if nanoseconds == 0L {
    return self
  }
  let seconds = checked_add_int64(
    self.secs,
    nanoseconds / nanoseconds_per_second,
  )
  let nanoseconds = nanoseconds % nanoseconds_per_second
  Duration::of(seconds~, nanoseconds~)
}

///|
/// Adds other duration to this duration, and returns a new duration.
pub fn Duration::add_duration(
  self : Duration,
  other : Duration,
) -> Duration raise Error {
  if other.is_zero() {
    return self
  }
  let mut nanos = self.nanos.to_int64() + other.nanos.to_int64()
  let mut secs = checked_add_int64(self.secs, other.secs)
  secs = checked_add_int64(secs, nanos / nanoseconds_per_second)
  nanos = nanos % nanoseconds_per_second
  if nanos < 0L {
    secs -= 1L
    nanos += nanoseconds_per_second
  }
  create(secs, nanos.to_int())
}

///|
pub fn Duration::op_add(
  self : Duration,
  other : Duration,
) -> Duration raise Error {
  self.add_duration(other)
}

///|
/// Returns a new duration with the specified amount of seconds.
pub fn Duration::with_seconds(self : Duration, seconds : Int64) -> Duration {
  if seconds == self.secs {
    return self
  }
  { secs: seconds, nanos: self.nanos }
}

///|
/// Returns a new duration with the specified nanosecond of second.
pub fn Duration::with_nanoseconds(
  self : Duration,
  nanoseconds : Int,
) -> Duration raise Error {
  if nanoseconds == self.nanos {
    return self
  }
  create(self.secs, nanoseconds)
}

///|
/// Converts this duration to the total length in nanoseconds.
pub fn Duration::to_nanoseconds(self : Duration) -> Int64 {
  self.secs * nanoseconds_per_second + self.nanos.to_int64()
}
