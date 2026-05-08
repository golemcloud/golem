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
  inspect(@time.PlainDateTime::of(2000, 1, 1), content="2000-01-01T00:00:00")
  inspect(
    @time.PlainDateTime::of(
      2000,
      1,
      1,
      hour=1,
      minute=2,
      second=3,
      nanosecond=400,
    ),
    content="2000-01-01T01:02:03.0000004",
  )
  inspect(@time.PlainDateTime::of(0, 1, 1), content="0000-01-01T00:00:00")
  inspect(
    @time.PlainDateTime::of(
      9999,
      12,
      31,
      hour=23,
      minute=59,
      second=59,
      nanosecond=999_999_999,
    ),
    content="9999-12-31T23:59:59.999999999",
  )
  inspect(
    @time.PlainDateTime::of(
      -9999,
      12,
      31,
      hour=23,
      minute=59,
      second=59,
      nanosecond=999_999_999,
    ),
    content="-9999-12-31T23:59:59.999999999",
  )
}

///|
test "from_unix_second" {
  inspect(
    @time.PlainDateTime::from_unix_second(1714227729L, 1000, @time.utc_offset),
    content="2024-04-27T14:22:09.000001",
  )
  inspect(
    @time.PlainDateTime::from_unix_second(
      1714227729L,
      1000,
      @time.ZoneOffset::from_seconds(8 * 60 * 60),
    ),
    content="2024-04-27T22:22:09.000001",
  )
  assert_true(
    (try? @time.PlainDateTime::from_unix_second(
      1714227729L, 1_000_000_000, @time.utc_offset,
    ))
    is Err(_),
  )
}

///|
test "from_string" {
  inspect(
    @time.PlainDateTime::from_string("0000-01-01T00:00:00"),
    content="0000-01-01T00:00:00",
  )
  inspect(
    @time.PlainDateTime::from_string("9999-12-31T23:59:59"),
    content="9999-12-31T23:59:59",
  )
  inspect(
    @time.PlainDateTime::from_string("-9999-12-31T23:59:59"),
    content="-9999-12-31T23:59:59",
  )
  inspect(
    @time.PlainDateTime::from_string("2000-01-01T00:00:00.12345"),
    content="2000-01-01T00:00:00.12345",
  )
  assert_true(
    (try? @time.PlainDateTime::from_string("9999-12-31T23:59:60")) is Err(_),
  )
  assert_true(
    (try? @time.PlainDateTime::from_string("10000-12-31T23:59:60")) is Err(_),
  )
  assert_true(
    (try? @time.PlainDateTime::from_string("2000-01-01 00:00:00")) is Err(_),
  )
  assert_true((try? @time.PlainDateTime::from_string("2000-01-01")) is Err(_))
  assert_true((try? @time.PlainDateTime::from_string("00:00:00")) is Err(_))
}

///|
test "getters" {
  let d = @time.PlainDateTime::of(
    2000,
    2,
    29,
    hour=1,
    minute=10,
    second=30,
    nanosecond=1000,
  )
  inspect(d.era(), content="CE")
  inspect(d.era_year(), content="2000")
  inspect(d.year(), content="2000")
  inspect(d.month(), content="2")
  inspect(d.day(), content="29")
  inspect(d.weekday(), content="Tuesday")
  inspect(d.ordinal(), content="60")
  inspect(d.days_in_year(), content="366")
  inspect(d.days_in_month(), content="29")
  inspect(d.days_in_week(), content="7")
  inspect(d.months_in_year(), content="12")
  inspect(d.hour(), content="1")
  inspect(d.minute(), content="10")
  inspect(d.second(), content="30")
  inspect(d.nanosecond(), content="1000")
}

///|
test "in_leap_year" {
  inspect(@time.PlainDateTime::of(2000, 1, 1).in_leap_year(), content="true")
  inspect(@time.PlainDateTime::of(2001, 1, 1).in_leap_year(), content="false")
}

///|
test "add_years" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.add_years(0L), content="2000-02-29T00:00:00")
  inspect(d.add_years(1L), content="2001-02-28T00:00:00")
  inspect(d.add_years(7999L), content="9999-02-28T00:00:00")
  assert_true((try? d.add_years(8000L)) is Err(_))
  inspect(d.add_years(-11999L), content="-9999-02-28T00:00:00")
  assert_true((try? d.add_years(-12000L)) is Err(_))
}

///|
test "add_months" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.add_months(0L), content="2000-02-29T00:00:00")
  inspect(d.add_months(1L), content="2000-03-29T00:00:00")
  inspect(d.add_months(12L * 7999L), content="9999-02-28T00:00:00")
  assert_true((try? d.add_months(12L * 8000L)) is Err(_))
  inspect(d.add_months(12L * -11999L), content="-9999-02-28T00:00:00")
  assert_true((try? d.add_months(12L * -12000L)) is Err(_))
}

///|
test "add_weeks" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.add_weeks(0L), content="2000-02-29T00:00:00")
  inspect(d.add_weeks(1L), content="2000-03-07T00:00:00")
  inspect(d.add_weeks(-1L), content="2000-02-22T00:00:00")
  inspect(d.add_weeks(3L), content="2000-03-21T00:00:00")
  inspect(d.add_weeks(-3L), content="2000-02-08T00:00:00")
  assert_true((try? d.add_weeks(@int64.max_value)) is Err(_))
  assert_true((try? d.add_weeks(@int64.min_value)) is Err(_))
}

///|
test "add_days" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.add_days(0L), content="2000-02-29T00:00:00")
  inspect(d.add_days(1L), content="2000-03-01T00:00:00")
  inspect(d.add_days(-1L), content="2000-02-28T00:00:00")
  inspect(d.add_days(365L), content="2001-02-28T00:00:00")
  inspect(d.add_days(-365L), content="1999-03-01T00:00:00")
  assert_true((try? d.add_days(@int64.max_value)) is Err(_))
  assert_true((try? d.add_days(@int64.min_value)) is Err(_))
}

///|
test "add_period" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(
    d.add_period(@time.Period::of(years=1)),
    content="2001-02-28T00:00:00",
  )
  inspect(
    d.add_period(@time.Period::of(months=1)),
    content="2000-03-29T00:00:00",
  )
  inspect(d.add_period(@time.Period::of(days=1)), content="2000-03-01T00:00:00")
  inspect(
    d.add_period(@time.Period::of(years=1, months=1, days=1)),
    content="2001-03-30T00:00:00",
  )
}

///|
test "add_hours" {
  let d = @time.PlainDateTime::of(0, 1, 1)
  inspect(d.add_hours(0L), content="0000-01-01T00:00:00")
  inspect(d.add_hours(1L), content="0000-01-01T01:00:00")
  inspect(d.add_hours(24L), content="0000-01-02T00:00:00")
  inspect(d.add_hours(24L * 30L), content="0000-01-31T00:00:00")
  inspect(d.add_hours(24L * 366L), content="0001-01-01T00:00:00")
}

///|
test "add_minutes" {
  let d = @time.PlainDateTime::of(0, 1, 1)
  inspect(d.add_minutes(0L), content="0000-01-01T00:00:00")
  inspect(d.add_minutes(1L), content="0000-01-01T00:01:00")
  inspect(d.add_minutes(60L), content="0000-01-01T01:00:00")
  inspect(d.add_minutes(24L * 60L), content="0000-01-02T00:00:00")
  inspect(d.add_minutes(24L * 60L * 366L), content="0001-01-01T00:00:00")
}

///|
test "add_seconds" {
  let d = @time.PlainDateTime::of(0, 1, 1)
  inspect(d.add_seconds(0L), content="0000-01-01T00:00:00")
  inspect(d.add_seconds(1L), content="0000-01-01T00:00:01")
  inspect(d.add_seconds(60L), content="0000-01-01T00:01:00")
  inspect(d.add_seconds(60L * 60L), content="0000-01-01T01:00:00")
  inspect(d.add_seconds(24L * 60L * 60L), content="0000-01-02T00:00:00")
  inspect(d.add_seconds(24L * 60L * 60L * 366L), content="0001-01-01T00:00:00")
}

///|
test "add_duration" {
  let d = @time.PlainDateTime::of(0, 1, 1)
  inspect(d.add_duration(@time.Duration::zero()), content="0000-01-01T00:00:00")
  inspect(
    d.add_duration(@time.Duration::of(hours=1L)),
    content="0000-01-01T01:00:00",
  )
  inspect(
    d.add_duration(@time.Duration::of(minutes=1L)),
    content="0000-01-01T00:01:00",
  )
  inspect(
    d.add_duration(@time.Duration::of(seconds=1L)),
    content="0000-01-01T00:00:01",
  )
  inspect(
    d.add_duration(@time.Duration::of(nanoseconds=100L)),
    content="0000-01-01T00:00:00.0000001",
  )
  inspect(
    d.add_duration(
      @time.Duration::of(hours=1L, minutes=10L, seconds=30L, nanoseconds=1000L),
    ),
    content="0000-01-01T01:10:30.000001",
  )
}

///|
test "with_year" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.with_year(2000), content="2000-02-29T00:00:00")
  inspect(d.with_year(2001), content="2001-02-28T00:00:00")
  inspect(d.with_year(2008), content="2008-02-29T00:00:00")
  inspect(d.with_year(0), content="0000-02-29T00:00:00")
  inspect(d.with_year(9999), content="9999-02-28T00:00:00")
  inspect(d.with_year(-9999), content="-9999-02-28T00:00:00")
  assert_true((try? d.with_year(10000)) is Err(_))
  assert_true((try? d.with_year(-10000)) is Err(_))
}

///|
test "with_month" {
  let d = @time.PlainDateTime::of(2000, 1, 31)
  inspect(d.with_month(1), content="2000-01-31T00:00:00")
  inspect(d.with_month(2), content="2000-02-29T00:00:00")
  inspect(d.with_month(4), content="2000-04-30T00:00:00")
  assert_true((try? d.with_month(0)) is Err(_))
  assert_true((try? d.with_month(13)) is Err(_))
}

///|
test "with_day" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.with_day(29), content="2000-02-29T00:00:00")
  inspect(d.with_day(1), content="2000-02-01T00:00:00")
  assert_true((try? d.with_day(0)) is Err(_))
  assert_true((try? d.with_day(30)) is Err(_))
}

///|
test "with_ordinal" {
  let d = @time.PlainDateTime::of(2000, 2, 29)
  inspect(d.with_ordinal(1), content="2000-01-01T00:00:00")
  inspect(d.with_ordinal(366), content="2000-12-31T00:00:00")
  assert_true((try? d.with_ordinal(0)) is Err(_))
  assert_true((try? d.with_ordinal(367)) is Err(_))
}

///|
test "with_hour" {
  let d = @time.PlainDateTime::of(2000, 1, 1, hour=1)
  inspect(d.with_hour(1), content="2000-01-01T01:00:00")
  inspect(d.with_hour(10), content="2000-01-01T10:00:00")
  inspect(d.with_hour(0), content="2000-01-01T00:00:00")
  inspect(d.with_hour(24), content="2000-01-01T24:00:00")
  assert_true((try? d.with_hour(-1)) is Err(_))
  assert_true((try? d.with_hour(25)) is Err(_))
}

///|
test "with_minute" {
  let d = @time.PlainDateTime::of(2000, 1, 1, minute=20)
  inspect(d.with_minute(20), content="2000-01-01T00:20:00")
  inspect(d.with_minute(10), content="2000-01-01T00:10:00")
  inspect(d.with_minute(0), content="2000-01-01T00:00:00")
  inspect(d.with_minute(59), content="2000-01-01T00:59:00")
  assert_true((try? d.with_minute(-1)) is Err(_))
  assert_true((try? d.with_minute(60)) is Err(_))
}

///|
test "with_second" {
  let d = @time.PlainDateTime::of(2000, 1, 1, second=30)
  inspect(d.with_second(30), content="2000-01-01T00:00:30")
  inspect(d.with_second(10), content="2000-01-01T00:00:10")
  inspect(d.with_second(0), content="2000-01-01T00:00:00")
  inspect(d.with_second(59), content="2000-01-01T00:00:59")
  assert_true((try? d.with_second(-1)) is Err(_))
  assert_true((try? d.with_second(60)) is Err(_))
}

///|
test "with_nanosecond" {
  let d = @time.PlainDateTime::of(2000, 1, 1, nanosecond=400)
  inspect(d.with_nanosecond(400), content="2000-01-01T00:00:00.0000004")
  inspect(d.with_nanosecond(100), content="2000-01-01T00:00:00.0000001")
  inspect(d.with_nanosecond(0), content="2000-01-01T00:00:00")
  inspect(
    d.with_nanosecond(999_999_999),
    content="2000-01-01T00:00:00.999999999",
  )
  assert_true((try? d.with_nanosecond(-1)) is Err(_))
  assert_true((try? d.with_nanosecond(1_000_000_000)) is Err(_))
}

///|
test "to_plain_date and to_plain_time" {
  let d = @time.PlainDateTime::of(
    2000,
    1,
    1,
    hour=1,
    minute=2,
    second=3,
    nanosecond=400,
  )
  inspect(d.to_plain_date(), content="2000-01-01")
  inspect(d.to_plain_time(), content="01:02:03.0000004")
}
