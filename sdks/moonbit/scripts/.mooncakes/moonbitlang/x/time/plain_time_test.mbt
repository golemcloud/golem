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
test "of" {
  inspect(@time.PlainTime::of(0, 0, 0, 0), content="00:00:00")
  inspect(@time.PlainTime::of(24, 0, 0, 0), content="24:00:00")
  inspect(
    @time.PlainTime::of(23, 59, 59, 999_999_999),
    content="23:59:59.999999999",
  )
  assert_true((try? @time.PlainTime::of(24, 0, 0, 1)) is Err(_))
  assert_true((try? @time.PlainTime::of(-1, 0, 0, 0)) is Err(_))
}

///|
test "from_string" {
  inspect(@time.PlainTime::from_string("00:00"), content="00:00:00")
  inspect(@time.PlainTime::from_string("23:59"), content="23:59:00")
  inspect(@time.PlainTime::from_string("24:00"), content="24:00:00")
  inspect(@time.PlainTime::from_string("23:59:59"), content="23:59:59")
  inspect(@time.PlainTime::from_string("01:00:00.123"), content="01:00:00.123")
  inspect(
    @time.PlainTime::from_string("01:00:00.0000123"),
    content="01:00:00.0000123",
  )
  assert_true((try? @time.PlainTime::from_string("1:00")) is Err(_))
  assert_true((try? @time.PlainTime::from_string("10:0")) is Err(_))
  assert_true((try? @time.PlainTime::from_string("100000")) is Err(_))
  assert_true((try? @time.PlainTime::from_string("123:00:00")) is Err(_))
  assert_true((try? @time.PlainTime::from_string("10:00:00.123zzz")) is Err(_))
  assert_true((try? @time.PlainTime::from_string("z1:00:00")) is Err(_))
}

///|
test "getters" {
  let time = @time.PlainTime::of(1, 2, 3, 400)
  inspect(time.hour(), content="1")
  inspect(time.minute(), content="2")
  inspect(time.second(), content="3")
  inspect(time.nanosecond(), content="400")
}

///|
test "second_of_day" {
  inspect(@time.PlainTime::of(0, 0, 0, 0).second_of_day(), content="0")
  inspect(@time.PlainTime::of(12, 30, 30, 0).second_of_day(), content="45030")
  inspect(@time.PlainTime::of(24, 0, 0, 0).second_of_day(), content="86400")
}

///|
test "nanosecond_of_day" {
  inspect(@time.PlainTime::of(0, 0, 0, 0).nanosecond_of_day(), content="0")
  inspect(
    @time.PlainTime::of(12, 30, 30, 123456789).nanosecond_of_day(),
    content="45030123456789",
  )
  inspect(
    @time.PlainTime::of(24, 0, 0, 0).nanosecond_of_day(),
    content="86400000000000",
  )
}

///|
test "from_second_of_day" {
  inspect(@time.PlainTime::from_second_of_day(0), content="00:00:00")
  inspect(@time.PlainTime::from_second_of_day(86400), content="24:00:00")
  inspect(@time.PlainTime::from_second_of_day(45030), content="12:30:30")
  inspect(@time.PlainTime::from_second_of_day(12345), content="03:25:45")
  assert_true((try? @time.PlainTime::from_second_of_day(-1)) is Err(_))
  assert_true((try? @time.PlainTime::from_second_of_day(86401)) is Err(_))
}

///|
test "from_nanosecond_of_day" {
  inspect(@time.PlainTime::from_nanosecond_of_day(0L), content="00:00:00")
  inspect(
    @time.PlainTime::from_nanosecond_of_day(86400000000000L),
    content="24:00:00",
  )
  inspect(
    @time.PlainTime::from_nanosecond_of_day(45030123456789L),
    content="12:30:30.123456789",
  )
  assert_true((try? @time.PlainTime::from_nanosecond_of_day(-1L)) is Err(_))
  assert_true(
    (try? @time.PlainTime::from_nanosecond_of_day(86400000000001L)) is Err(_),
  )
}

///|
test "add_hours" {
  let time = @time.PlainTime::of(10, 0, 0, 0)
  inspect(time.add_hours(-1L), content="09:00:00")
  inspect(time.add_hours(0L), content="10:00:00")
  inspect(time.add_hours(1L), content="11:00:00")
  inspect(time.add_hours(12L), content="22:00:00")
  inspect(time.add_hours(24L), content="10:00:00")
  inspect(time.add_hours(25L), content="11:00:00")
  inspect(time.add_hours(26L), content="12:00:00")
  inspect(time.add_hours(27L), content="13:00:00")
  inspect(time.add_hours(28L), content="14:00:00")
  inspect(time.add_hours(29L), content="15:00:00")
  inspect(time.add_hours(30L), content="16:00:00")
  inspect(time.add_hours(31L), content="17:00:00")
  inspect(time.add_hours(32L), content="18:00:00")
  inspect(time.add_hours(33L), content="19:00:00")
  inspect(time.add_hours(34L), content="20:00:00")
  inspect(time.add_hours(35L), content="21:00:00")
  inspect(time.add_hours(36L), content="22:00:00")
  inspect(time.add_hours(37L), content="23:00:00")
  inspect(time.add_hours(38L), content="00:00:00")
  inspect(time.add_hours(39L), content="01:00:00")
  inspect(time.add_hours(40L), content="02:00:00")
  inspect(time.add_hours(@int64.min_value), content="02:00:00")
  inspect(time.add_hours(@int64.max_value), content="17:00:00")
}

///|
test "add_minutes" {
  let time = @time.PlainTime::of(0, 30, 0, 0)
  inspect(time.add_minutes(-1L), content="00:29:00")
  inspect(time.add_minutes(0L), content="00:30:00")
  inspect(time.add_minutes(1L), content="00:31:00")
  inspect(time.add_minutes(30L), content="01:00:00")
  inspect(time.add_minutes(360L), content="06:30:00")
  inspect(time.add_minutes(720L), content="12:30:00")
  inspect(time.add_minutes(1234L), content="21:04:00")
  inspect(time.add_minutes(1440L), content="00:30:00")
  inspect(time.add_minutes(2345L), content="15:35:00")
  inspect(time.add_minutes(2880L), content="00:30:00")
  inspect(time.add_minutes(@int64.min_value), content="06:22:00")
  inspect(time.add_minutes(@int64.max_value), content="18:37:00")
}

///|
test "add_seconds" {
  let time = @time.PlainTime::of(0, 0, 30, 0)
  inspect(time.add_seconds(-1L), content="00:00:29")
  inspect(time.add_seconds(0L), content="00:00:30")
  inspect(time.add_seconds(1L), content="00:00:31")
  inspect(time.add_seconds(30L), content="00:01:00")
  inspect(time.add_seconds(60L), content="00:01:30")
  inspect(time.add_seconds(12345678L), content="21:21:48")
  inspect(time.add_seconds(@int64.min_value), content="08:30:22")
  inspect(time.add_seconds(@int64.max_value), content="15:30:37")
  inspect(time.add_seconds(24L * 60L * 60L), content="00:00:30")
}

///|
test "with_hour" {
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_hour(1),
    content="01:20:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_hour(10),
    content="10:20:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_hour(0),
    content="00:20:30.0000004",
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_hour(-1)) is Err(_),
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_hour(24)) is Err(_),
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_hour(25)) is Err(_),
  )
}

///|
test "with_minute" {
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_minute(20),
    content="01:20:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_minute(10),
    content="01:10:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_minute(0),
    content="01:00:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_minute(59),
    content="01:59:30.0000004",
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_minute(-1)) is Err(_),
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_minute(60)) is Err(_),
  )
}

///|
test "with_second" {
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_second(30),
    content="01:20:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_second(10),
    content="01:20:10.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_second(0),
    content="01:20:00.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_second(59),
    content="01:20:59.0000004",
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_second(-1)) is Err(_),
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_second(60)) is Err(_),
  )
}

///|
test "with_nanosecond" {
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(400),
    content="01:20:30.0000004",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(100),
    content="01:20:30.0000001",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(0),
    content="01:20:30",
  )
  inspect(
    @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(999_999_999),
    content="01:20:30.999999999",
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(-1)) is Err(_),
  )
  assert_true(
    (try? @time.PlainTime::of(1, 20, 30, 400).with_nanosecond(1_000_000_000))
    is Err(_),
  )
}

///|
test "until" {
  let time = @time.PlainTime::of(12, 0, 0, 0)
  inspect(
    time.until(@time.PlainTime::of(13, 30, 30, 1_000_000)),
    content="PT1H30M30.001S",
  )
  inspect(
    time.until(@time.PlainTime::of(11, 30, 30, 1_000_000)),
    content="PT-29M-29.999S",
  )
}
