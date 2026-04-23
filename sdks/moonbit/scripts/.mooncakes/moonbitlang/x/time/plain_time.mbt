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
/// A time without a time zone
struct PlainTime {
  hour : Int
  minute : Int
  second : Int
  nanosecond : Int
} derive(Eq, Compare)

///|
/// Creates a PlainTime from the hour, minute, second and nanosecond.
pub fn PlainTime::of(
  hour : Int,
  minute : Int,
  second : Int,
  nanosecond : Int,
) -> PlainTime raise Error {
  if validate_time(hour, minute, second, nanosecond) {
    { hour, minute, second, nanosecond }
  } else {
    fail(invalid_time_err)
  }
}

///|
/// Creates a PlainTime from a string, like '10:20:30.45678'.
pub fn PlainTime::from_string(str : String) -> PlainTime raise Error {
  let (hour, minute, second, nanosecond) = lexmatch str with longest {
    (("[0-9]{2}" as hour_str) ":" ("[0-9]{2}" as minute_str)) =>
      (@strconv.parse_int(hour_str), @strconv.parse_int(minute_str), 0, 0)
    (
      ("[0-9]{2}" as hour_str)
      ":"
      ("[0-9]{2}" as minute_str)
      ":"
      ("[0-9]{2}" as second_str)
    ) =>
      (
        @strconv.parse_int(hour_str),
        @strconv.parse_int(minute_str),
        @strconv.parse_int(second_str),
        0,
      )
    (
      ("[0-9]{2}" as hour_str)
      ":"
      ("[0-9]{2}" as minute_str)
      ":"
      ("[0-9]{2}" as second_str)
      "."
      ("[0-9]{1,9}" as nanosecond_str)
    ) =>
      (
        @strconv.parse_int(hour_str),
        @strconv.parse_int(minute_str),
        @strconv.parse_int(second_str),
        @strconv.parse_int(add_suffix_zero(nanosecond_str.to_string(), 9)),
      )
    _ => fail(invalid_time_err)
  }
  PlainTime::of(hour, minute, second, nanosecond)
}

///|
/// Returns a string representing the time.
pub fn PlainTime::to_string(self : PlainTime) -> String {
  let buf = StringBuilder::new(size_hint=8)
  buf.write_string(add_prefix_zero(self.hour.to_string(), 2))
  buf.write_char(':')
  buf.write_string(add_prefix_zero(self.minute.to_string(), 2))
  buf.write_char(':')
  buf.write_string(add_prefix_zero(self.second.to_string(), 2))
  if self.nanosecond > 0 {
    buf.write_string(format_nanos(self.nanosecond.to_int64()).to_string())
  }
  buf.to_string()
}

///|
pub impl Show for PlainTime with output(self : PlainTime, logger : &Logger) -> Unit {
  logger.write_string(self.to_string())
}

///|
/// Returns the hour of this time.
pub fn PlainTime::hour(self : PlainTime) -> Int {
  self.hour
}

///|
/// Returns the minute of this time.
pub fn PlainTime::minute(self : PlainTime) -> Int {
  self.minute
}

///|
/// Returns the second of this time.
pub fn PlainTime::second(self : PlainTime) -> Int {
  self.second
}

///|
/// Returns the nanosecond of this time.
pub fn PlainTime::nanosecond(self : PlainTime) -> Int {
  self.nanosecond
}

///|
/// Returns the total seconds of this time.
pub fn PlainTime::second_of_day(self : PlainTime) -> Int {
  let sec = self.hour * seconds_per_hour.to_int() +
    self.minute * seconds_per_minute.to_int() +
    self.second
  sec
}

///|
/// Returns the total nanoseconds of this time.
pub fn PlainTime::nanosecond_of_day(self : PlainTime) -> Int64 {
  let sec = self.second_of_day()
  sec.to_int64() * nanoseconds_per_second + self.nanosecond.to_int64()
}

///|
/// Creates a PlainTime from the total seconds of the day.
pub fn PlainTime::from_second_of_day(second : Int) -> PlainTime raise Error {
  if second < 0 || second > seconds_per_day.to_int() {
    fail(invalid_time_err)
  }
  let mut sec = second.to_int64()
  let hour = sec / seconds_per_hour
  sec -= hour * seconds_per_hour
  let minute = sec / seconds_per_minute
  sec -= minute * seconds_per_minute
  PlainTime::of(hour.to_int(), minute.to_int(), sec.to_int(), 0)
}

///|
/// Creates a PlainTime from the total nanoseconds of the day.
pub fn PlainTime::from_nanosecond_of_day(
  nanosecond : Int64,
) -> PlainTime raise Error {
  if nanosecond < 0L || nanosecond > nanoseconds_per_day {
    fail(invalid_time_err)
  }
  let mut nanos = nanosecond
  let hour = nanos / nanoseconds_per_hour
  nanos -= hour * nanoseconds_per_hour
  let minute = nanos / nanoseconds_per_minute
  nanos -= minute * nanoseconds_per_minute
  let second = nanos / nanoseconds_per_second
  nanos -= second * nanoseconds_per_second
  PlainTime::of(hour.to_int(), minute.to_int(), second.to_int(), nanos.to_int())
}

///|
/// Adds specified hours to this time, and returns a new time.
pub fn PlainTime::add_hours(
  self : PlainTime,
  hours : Int64,
) -> PlainTime raise Error {
  if hours == 0L {
    return self
  }
  let new_hour = (hours % hours_per_day + self.hour.to_int64()) % hours_per_day
  PlainTime::of(new_hour.to_int(), self.minute, self.second, self.nanosecond)
}

///|
/// Adds specified minutes to this time, and returns a new time.
pub fn PlainTime::add_minutes(
  self : PlainTime,
  minutes : Int64,
) -> PlainTime raise Error {
  if minutes == 0L {
    return self
  }
  let mins = self.hour.to_int64() * minutes_per_hour + self.minute.to_int64()
  let new_mins = (minutes % minutes_per_day + mins + minutes_per_day) %
    minutes_per_day
  if mins == new_mins {
    return self
  }
  let new_hour = new_mins / minutes_per_hour
  let new_minute = new_mins % minutes_per_hour
  PlainTime::of(
    new_hour.to_int(),
    new_minute.to_int(),
    self.second,
    self.nanosecond,
  )
}

///|
/// Adds specified seconds to this time, and returns a new time.
pub fn PlainTime::add_seconds(
  self : PlainTime,
  seconds : Int64,
) -> PlainTime raise Error {
  if seconds == 0L {
    return self
  }
  let secs_of_day = self.second_of_day().to_int64()
  let new_secs_of_day = (
      seconds % seconds_per_day + secs_of_day + seconds_per_day
    ) %
    seconds_per_day
  if secs_of_day == new_secs_of_day {
    return self
  }
  let new_hour = new_secs_of_day / seconds_per_hour
  let new_minute = new_secs_of_day / seconds_per_minute % minutes_per_hour
  let new_second = new_secs_of_day % seconds_per_minute
  PlainTime::of(
    new_hour.to_int(),
    new_minute.to_int(),
    new_second.to_int(),
    self.nanosecond,
  )
}

///|
/// Adds specified nanoseconds to this time, and returns a new time.
pub fn PlainTime::add_nanoseconds(
  self : PlainTime,
  nanoseconds : Int64,
) -> PlainTime raise Error {
  if nanoseconds == 0L {
    return self
  }
  let nanos = self.nanosecond_of_day()
  let new_nanos = (
      nanoseconds % nanoseconds_per_day + nanos + nanoseconds_per_day
    ) %
    nanoseconds_per_day
  if nanos == new_nanos {
    return self
  }
  PlainTime::from_nanosecond_of_day(new_nanos)
}

///|
/// Adds a duration to this time, and returns a new time.
pub fn PlainTime::add_duration(
  self : PlainTime,
  duration : Duration,
) -> PlainTime raise Error {
  if duration.is_zero() {
    return self
  }
  self.add_nanoseconds(duration.to_nanoseconds())
}

///|
/// Returns a new time with the specified hour.
pub fn PlainTime::with_hour(
  self : PlainTime,
  hour : Int,
) -> PlainTime raise Error {
  if hour == self.hour {
    return self
  }
  PlainTime::of(hour, self.minute, self.second, self.nanosecond)
}

///|
/// Returns a new time with the specified minute.
pub fn PlainTime::with_minute(
  self : PlainTime,
  minute : Int,
) -> PlainTime raise Error {
  if minute == self.minute {
    return self
  }
  PlainTime::of(self.hour, minute, self.second, self.nanosecond)
}

///|
/// Returns a new time with the specified second.
pub fn PlainTime::with_second(
  self : PlainTime,
  second : Int,
) -> PlainTime raise Error {
  if second == self.second {
    return self
  }
  PlainTime::of(self.hour, self.minute, second, self.nanosecond)
}

///|
/// Returns a new time with the specified nanosecond.
pub fn PlainTime::with_nanosecond(
  self : PlainTime,
  nanosecond : Int,
) -> PlainTime raise Error {
  if nanosecond == self.nanosecond {
    return self
  }
  PlainTime::of(self.hour, self.minute, self.second, nanosecond)
}

///|
/// Returns the duration between this time and another time.
pub fn PlainTime::until(
  self : PlainTime,
  end : PlainTime,
) -> Duration raise Error {
  let nanoseconds = end.nanosecond_of_day() - self.nanosecond_of_day()
  Duration::of(nanoseconds~)
}

///|
/// Combines this time with a date to creates a PlainDateTime
pub fn PlainTime::at_date(self : PlainTime, date : PlainDate) -> PlainDateTime {
  PlainDateTime::{ date, time: self }
}

// *****************************
// * internal helper functions *
// *****************************

///|
let min_hour = 0

///|
let max_hour = 23

///|
let min_minute = 0

///|
let max_minute = 59

///|
let min_second = 0

///|
let max_second = 59

///|
let min_nanosecond = 0

///|
let max_nanosecond = 999_999_999

///|
let hours_per_day = 24L

///|
let minutes_per_hour = 60L

///|
let minutes_per_day : Int64 = 24L * minutes_per_hour

///|
let seconds_per_minute = 60L

///|
let seconds_per_hour : Int64 = seconds_per_minute * 60L

///|
let seconds_per_day : Int64 = seconds_per_hour * 24L

///|
let nanoseconds_per_second = 1_000_000_000L

///|
let nanoseconds_per_minute : Int64 = nanoseconds_per_second * seconds_per_minute

///|
let nanoseconds_per_hour : Int64 = nanoseconds_per_minute * minutes_per_hour

///|
let nanoseconds_per_day : Int64 = nanoseconds_per_second * seconds_per_day

///|
fn validate_time(h : Int, m : Int, s : Int, ns : Int) -> Bool {
  let valid_hour = validate_hour(h)
  let valid_minute = validate_minute(m)
  let valid_second = validate_second(s)
  let valid_nano = validate_nano(ns)
  let zero = h == 24 && m == 0 && s == 0 && ns == 0
  (valid_hour && valid_minute && valid_second && valid_nano) || zero
}

///|
fn validate_hour(h : Int) -> Bool {
  h >= min_hour && h <= max_hour
}

///|
fn validate_minute(m : Int) -> Bool {
  m >= min_minute && m <= max_minute
}

///|
fn validate_second(s : Int) -> Bool {
  s >= min_second && s <= max_second
}

///|
fn validate_nano(n : Int) -> Bool {
  n >= min_nanosecond && n <= max_nanosecond
}
