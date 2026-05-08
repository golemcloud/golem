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
/// Time offset from UTC.
struct ZoneOffset {
  id : String // e.g. "Z", "+08:00", "-05:30", etc.
  abbrev : String // e.g. "PST", "CST", "EST", "UTC", etc.
  seconds : Int
  dst : Bool // daylight savings time
}

///|
let max_offset_hours = 18

///|
let min_offset_hours = -18

///|
let max_offset_seconds : Int = max_offset_hours * seconds_per_hour.to_int()

///|
let min_offset_seconds : Int = min_offset_hours * seconds_per_hour.to_int()

///|
/// UTC+0 offset.
pub let utc_offset : ZoneOffset = ZoneOffset::{
  id: "Z",
  abbrev: "UTC",
  seconds: 0,
  dst: false,
}

///|
/// Creates a offset from hours, minutes and seconds.
pub fn ZoneOffset::of(
  hours? : Int = 0,
  minutes? : Int = 0,
  seconds? : Int = 0,
  abbrev? : String = "",
  dst? : Bool = false,
) -> ZoneOffset raise Error {
  validate_offset(hours, minutes, seconds)
  let secs = hours * seconds_per_hour.to_int() +
    minutes * seconds_per_minute.to_int() +
    seconds
  ZoneOffset::from_seconds(secs, abbrev~, dst~)
}

///|
/// Creates a offset from seconds.
pub fn ZoneOffset::from_seconds(
  seconds : Int,
  abbrev? : String = "",
  dst? : Bool = false,
) -> ZoneOffset raise Error {
  if seconds == 0 {
    return utc_offset
  }
  if seconds < min_offset_seconds || seconds > max_offset_seconds {
    fail(invalid_offset_err)
  }
  { id: create_id(seconds), abbrev, seconds, dst }
}

///|
/// Returns the offset id, e.g. "Z", "+08:00", "-05:30", etc.
pub fn ZoneOffset::to_string(self : ZoneOffset) -> String {
  self.id
}

///|
/// Returns the zone offset abbreviation, e.g. "PST", "CST", "EST", "UTC", etc.
pub fn ZoneOffset::abbreviation(self : ZoneOffset) -> String {
  self.abbrev
}

///|
pub impl Show for ZoneOffset with output(self : ZoneOffset, logger : &Logger) {
  logger.write_string(self.to_string())
}

///|
pub impl Eq for ZoneOffset with equal(self : ZoneOffset, other : ZoneOffset) -> Bool {
  self.seconds == other.seconds
}

///|
pub impl Compare for ZoneOffset with compare(
  self : ZoneOffset,
  other : ZoneOffset,
) -> Int {
  other.seconds - self.seconds
}

///|
/// Returns the offset id.
pub fn ZoneOffset::id(self : ZoneOffset) -> String {
  self.id
}

///|
/// Returns the total seconds of this offset.
pub fn ZoneOffset::seconds(self : ZoneOffset) -> Int {
  self.seconds
}

///|
/// Checks if this offset is daylight saving time.
pub fn ZoneOffset::is_dst(self : ZoneOffset) -> Bool {
  self.dst
}

///|
fn create_id(secs : Int) -> String {
  if secs == 0 {
    return "Z"
  }
  let abs_secs = secs.abs()
  let offset_hours = abs_secs / seconds_per_hour.to_int()
  let offset_mins = abs_secs %
    seconds_per_hour.to_int() /
    seconds_per_minute.to_int()
  let offset_secs = abs_secs % seconds_per_minute.to_int()
  let buf = StringBuilder::new(size_hint=9)
  buf.write_char(if secs < 0 { '-' } else { '+' })
  buf.write_string(add_prefix_zero(offset_hours.to_string(), 2))
  buf.write_char(':')
  buf.write_string(add_prefix_zero(offset_mins.to_string(), 2))
  if offset_secs > 0 {
    buf.write_char(':')
    buf.write_string(add_prefix_zero(offset_secs.to_string(), 2))
  }
  buf.to_string()
}

///|
fn validate_offset(h : Int, m : Int, s : Int) -> Unit raise Error {
  if h < min_offset_hours || h > max_offset_hours {
    fail(invalid_offset_err)
  }
  if m < -59 || m > 59 {
    fail(invalid_offset_err)
  }
  if s < -59 || s > 59 {
    fail(invalid_offset_err)
  }
  if (h > 0 && (m < 0 || s < 0)) || (h < 0 && (m > 0 || s > 0)) {
    fail(invalid_offset_err)
  }
  if (m > 0 && s < 0) || (m < 0 && s > 0) {
    fail(invalid_offset_err)
  }
  if h.abs() == 18 && (m != 0 || s != 0) {
    fail(invalid_offset_err)
  }
  ()
}
