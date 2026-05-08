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
  inspect(@time.PlainDate::of(0, 1, 1), content="0000-01-01")
  inspect(@time.PlainDate::of(1, 1, 1), content="0001-01-01")
  inspect(@time.PlainDate::of(2000, 2, 29), content="2000-02-29")
  inspect(@time.PlainDate::of(9999, 1, 1), content="9999-01-01")
  inspect(@time.PlainDate::of(-1, 1, 1), content="-0001-01-01")
  inspect(@time.PlainDate::of(-2000, 2, 29), content="-2000-02-29")
  inspect(@time.PlainDate::of(-9999, 1, 1), content="-9999-01-01")
  assert_true((try? @time.PlainDate::of(2001, 2, 29)) is Err(_))
  assert_true((try? @time.PlainDate::of(2001, 1, 32)) is Err(_))
  assert_true((try? @time.PlainDate::of(10000, 1, 1)) is Err(_))
}

///|
test "from_year_ord" {
  inspect(@time.PlainDate::from_year_ord(0, 1), content="0000-01-01")
  inspect(@time.PlainDate::from_year_ord(1, 365), content="0001-12-31")
  inspect(@time.PlainDate::from_year_ord(-1, 365), content="-0001-12-31")
  inspect(@time.PlainDate::from_year_ord(2000, 366), content="2000-12-31")
  inspect(@time.PlainDate::from_year_ord(-2000, 366), content="-2000-12-31")
  inspect(@time.PlainDate::from_year_ord(9999, 365), content="9999-12-31")
  inspect(@time.PlainDate::from_year_ord(-9999, 365), content="-9999-12-31")
  assert_true((try? @time.PlainDate::from_year_ord(2001, 366)) is Err(_))
  assert_true((try? @time.PlainDate::from_year_ord(-2001, 366)) is Err(_))
  assert_true((try? @time.PlainDate::from_year_ord(10000, 365)) is Err(_))
  assert_true((try? @time.PlainDate::from_year_ord(-10000, 365)) is Err(_))
}

///|
test "from_string" {
  inspect(@time.PlainDate::from_string("9999-01-01"), content="9999-01-01")
  inspect(@time.PlainDate::from_string("0000-01-01"), content="0000-01-01")
  inspect(@time.PlainDate::from_string("0001-01-01"), content="0001-01-01")
  inspect(@time.PlainDate::from_string("-9999-01-01"), content="-9999-01-01")
  assert_true((try? @time.PlainDate::from_string("10000-01-01")) is Err(_))
  assert_true((try? @time.PlainDate::from_string("-10000-01-01")) is Err(_))
  assert_true((try? @time.PlainDate::from_string("-01-01")) is Err(_))
  assert_true((try? @time.PlainDate::from_string("12-31")) is Err(_))
  assert_true((try? @time.PlainDate::from_string("-01")) is Err(_))
}

///|
test "compare" {
  inspect(
    @time.PlainDate::of(2000, 1, 1) == @time.PlainDate::of(2000, 1, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2001, 1, 1) > @time.PlainDate::of(2000, 1, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 1) < @time.PlainDate::of(2001, 1, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2000, 2, 1) > @time.PlainDate::of(2000, 1, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 1) < @time.PlainDate::of(2000, 2, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 2) > @time.PlainDate::of(2000, 1, 1),
    content="true",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 1) < @time.PlainDate::of(2000, 1, 2),
    content="true",
  )
}

///|
test "era" {
  inspect(@time.PlainDate::of(-9999, 1, 1).era(), content="BCE")
  inspect(@time.PlainDate::of(-1, 1, 1).era(), content="BCE")
  inspect(@time.PlainDate::of(0, 1, 1).era(), content="BCE")
  inspect(@time.PlainDate::of(1, 1, 1).era(), content="CE")
  inspect(@time.PlainDate::of(9999, 1, 1).era(), content="CE")
}

///|
test "era_year" {
  inspect(@time.PlainDate::of(-9999, 1, 1).era_year(), content="10000")
  inspect(@time.PlainDate::of(-1, 1, 1).era_year(), content="2")
  inspect(@time.PlainDate::of(0, 1, 1).era_year(), content="1")
  inspect(@time.PlainDate::of(1, 1, 1).era_year(), content="1")
  inspect(@time.PlainDate::of(9999, 1, 1).era_year(), content="9999")
}

///|
test "getters" {
  let date = @time.PlainDate::of(2000, 12, 31)
  inspect(date.year(), content="2000")
  inspect(date.month(), content="12")
  inspect(date.day(), content="31")
  inspect(date.ordinal(), content="366")
  let date = @time.PlainDate::of(-2000, 12, 31)
  inspect(date.year(), content="-2000")
  inspect(date.month(), content="12")
  inspect(date.day(), content="31")
  inspect(date.ordinal(), content="366")
}

///|
test "weekday" {
  inspect(@time.PlainDate::of(2000, 1, 1).weekday(), content="Saturday")
  inspect(@time.PlainDate::of(2000, 1, 2).weekday(), content="Sunday")
  inspect(@time.PlainDate::of(2000, 1, 3).weekday(), content="Monday")
  inspect(@time.PlainDate::of(2000, 1, 4).weekday(), content="Tuesday")
  inspect(@time.PlainDate::of(2000, 1, 5).weekday(), content="Wednesday")
  inspect(@time.PlainDate::of(2000, 1, 6).weekday(), content="Thursday")
  inspect(@time.PlainDate::of(2000, 1, 7).weekday(), content="Friday")
}

///|
test "ordinal" {
  inspect(@time.PlainDate::of(1, 1, 1).ordinal(), content="1")
  inspect(@time.PlainDate::of(2000, 12, 31).ordinal(), content="366")
  inspect(@time.PlainDate::of(2001, 12, 31).ordinal(), content="365")
}

///|
test "days_in_week" {
  inspect(@time.PlainDate::of(1, 1, 1).days_in_week(), content="7")
}

///|
test "days_in_month" {
  inspect(@time.PlainDate::of(2000, 1, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 2, 1).days_in_month(), content="29")
  inspect(@time.PlainDate::of(2000, 3, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 4, 1).days_in_month(), content="30")
  inspect(@time.PlainDate::of(2000, 5, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 6, 1).days_in_month(), content="30")
  inspect(@time.PlainDate::of(2000, 7, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 8, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 9, 1).days_in_month(), content="30")
  inspect(@time.PlainDate::of(2000, 10, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2000, 11, 1).days_in_month(), content="30")
  inspect(@time.PlainDate::of(2000, 12, 1).days_in_month(), content="31")
  inspect(@time.PlainDate::of(2001, 2, 1).days_in_month(), content="28")
}

///|
test "days_in_year" {
  inspect(@time.PlainDate::of(2000, 1, 1).days_in_year(), content="366")
  inspect(@time.PlainDate::of(2001, 1, 1).days_in_year(), content="365")
  inspect(@time.PlainDate::of(-2000, 1, 1).days_in_year(), content="366")
  inspect(@time.PlainDate::of(-2001, 1, 1).days_in_year(), content="365")
}

///|
test "months_in_year" {
  inspect(@time.PlainDate::of(2000, 1, 1).months_in_year(), content="12")
}

///|
test "in_leap_year" {
  inspect(@time.PlainDate::of(2000, 1, 1).in_leap_year(), content="true")
  inspect(@time.PlainDate::of(2000, 1, 1).in_leap_year(), content="true")
  inspect(@time.PlainDate::of(2001, 1, 1).in_leap_year(), content="false")
  inspect(@time.PlainDate::of(-2000, 1, 1).in_leap_year(), content="true")
  inspect(@time.PlainDate::of(-2001, 1, 1).in_leap_year(), content="false")
  inspect(@time.PlainDate::of(-1, 1, 1).in_leap_year(), content="false")
  inspect(@time.PlainDate::of(0, 1, 1).in_leap_year(), content="true")
  inspect(@time.PlainDate::of(1, 1, 1).in_leap_year(), content="false")
}

///|
test "add_years" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.add_years(0L), content="2000-02-29")
  inspect(d.add_years(1L), content="2001-02-28")
  inspect(d.add_years(-1L), content="1999-02-28")
  inspect(d.add_years(-1999L), content="0001-02-28")
  inspect(d.add_years(7999L), content="9999-02-28")
  inspect(d.add_years(-2001L), content="-0001-02-28")
  inspect(d.add_years(-9999L), content="-7999-02-28")
  assert_true((try? d.add_years(8000L)) is Err(_))
  assert_true((try? d.add_years(@int64.max_value)) is Err(_))
  inspect(d.add_years(@int64.min_value), content="2000-02-29")
}

///|
test "add_months" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.add_months(0L), content="2000-02-29")
  inspect(d.add_months(1L), content="2000-03-29")
  inspect(d.add_months(-1L), content="2000-01-29")
  inspect(d.add_months(-12L), content="1999-02-28")
  inspect(d.add_months(12L), content="2001-02-28")
  inspect(d.add_months(24L), content="2002-02-28")
  inspect(d.add_months(12L * -2000L), content="0000-02-29")
  assert_true((try? d.add_months(12L * 8000L)) is Err(_))
  assert_true((try? d.add_months(@int64.max_value)) is Err(_))
  assert_true((try? d.add_months(@int64.min_value)) is Err(_))
  let d = @time.PlainDate::of(-2000, 2, 29)
  inspect(d.add_months(0L), content="-2000-02-29")
  inspect(d.add_months(1L), content="-2000-03-29")
  inspect(d.add_months(-1L), content="-2000-01-29")
  inspect(d.add_months(-12L), content="-2001-02-28")
  inspect(d.add_months(12L), content="-1999-02-28")
  inspect(d.add_months(24L), content="-1998-02-28")
  inspect(d.add_months(12L * 10000L), content="8000-02-29")
  assert_true((try? d.add_months(12L * -8000L)) is Err(_))
  assert_true((try? d.add_months(@int64.max_value)) is Err(_))
  assert_true((try? d.add_months(@int64.min_value)) is Err(_))
}

///|
test "add_weeks" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.add_weeks(0L), content="2000-02-29")
  inspect(d.add_weeks(1L), content="2000-03-07")
  inspect(d.add_weeks(-1L), content="2000-02-22")
  inspect(d.add_weeks(3L), content="2000-03-21")
  inspect(d.add_weeks(-3L), content="2000-02-08")
  assert_true((try? d.add_weeks(@int64.max_value)) is Err(_))
  assert_true((try? d.add_weeks(@int64.min_value)) is Err(_))
  let d = @time.PlainDate::of(-2000, 2, 29)
  inspect(d.add_weeks(0L), content="-2000-02-29")
  inspect(d.add_weeks(1L), content="-2000-03-07")
  inspect(d.add_weeks(-1L), content="-2000-02-22")
  inspect(d.add_weeks(3L), content="-2000-03-21")
  inspect(d.add_weeks(-3L), content="-2000-02-08")
  assert_true((try? d.add_weeks(@int64.max_value)) is Err(_))
  assert_true((try? d.add_weeks(@int64.min_value)) is Err(_))
}

///|
test "add_days" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.add_days(0L), content="2000-02-29")
  inspect(d.add_days(1L), content="2000-03-01")
  inspect(d.add_days(-1L), content="2000-02-28")
  inspect(d.add_days(365L), content="2001-02-28")
  inspect(d.add_days(-365L), content="1999-03-01")
  assert_true((try? d.add_days(@int64.max_value)) is Err(_))
  assert_true((try? d.add_days(@int64.min_value)) is Err(_))
  let d = @time.PlainDate::of(-2000, 2, 29)
  inspect(d.add_days(0L), content="-2000-02-29")
  inspect(d.add_days(1L), content="-2000-03-01")
  inspect(d.add_days(-1L), content="-2000-02-28")
  inspect(d.add_days(365L), content="-1999-02-28")
  inspect(d.add_days(-365L), content="-2001-03-01")
  assert_true((try? d.add_days(@int64.max_value)) is Err(_))
  assert_true((try? d.add_days(@int64.min_value)) is Err(_))
}

///|
test "add_period" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.add_period(@time.Period::of(years=1)), content="2001-02-28")
  inspect(d.add_period(@time.Period::of(months=1)), content="2000-03-29")
  inspect(d.add_period(@time.Period::of(days=1)), content="2000-03-01")
  inspect(
    d.add_period(@time.Period::of(years=1, months=1, days=1)),
    content="2001-03-30",
  )
}

///|
test "with_year" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.with_year(2000), content="2000-02-29")
  inspect(d.with_year(2001), content="2001-02-28")
  inspect(d.with_year(2008), content="2008-02-29")
  inspect(d.with_year(0), content="0000-02-29")
  inspect(d.with_year(9999), content="9999-02-28")
  inspect(d.with_year(-9999), content="-9999-02-28")
  assert_true((try? d.with_year(10000)) is Err(_))
  assert_true((try? d.with_year(-10000)) is Err(_))
}

///|
test "with_month" {
  let d = @time.PlainDate::of(2000, 1, 31)
  inspect(d.with_month(1), content="2000-01-31")
  inspect(d.with_month(2), content="2000-02-29")
  inspect(d.with_month(4), content="2000-04-30")
  assert_true((try? d.with_month(0)) is Err(_))
  assert_true((try? d.with_month(13)) is Err(_))
}

///|
test "with_day" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.with_day(29), content="2000-02-29")
  inspect(d.with_day(1), content="2000-02-01")
  assert_true((try? d.with_day(0)) is Err(_))
  assert_true((try? d.with_day(30)) is Err(_))
}

///|
test "with_ordinal" {
  let d = @time.PlainDate::of(2000, 2, 29)
  inspect(d.with_ordinal(1), content="2000-01-01")
  inspect(d.with_ordinal(366), content="2000-12-31")
  assert_true((try? d.with_ordinal(0)) is Err(_))
  assert_true((try? d.with_ordinal(367)) is Err(_))
}

///|
test "until" {
  inspect(
    @time.PlainDate::of(1970, 1, 1).until(@time.PlainDate::of(2000, 1, 1)),
    content="P30Y",
  )
  inspect(
    @time.PlainDate::of(2000, 2, 1).until(@time.PlainDate::of(2000, 1, 15)),
    content="P-17D",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 15).until(@time.PlainDate::of(2000, 2, 1)),
    content="P17D",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 15).until(@time.PlainDate::of(2000, 3, 1)),
    content="P1M15D",
  )
  inspect(
    @time.PlainDate::of(2000, 1, 1).until(@time.PlainDate::of(2001, 2, 15)),
    content="P1Y1M14D",
  )
  inspect(
    @time.PlainDate::of(2001, 2, 15).until(@time.PlainDate::of(2000, 1, 1)),
    content="P-1Y-1M-14D",
  )
  inspect(
    @time.PlainDate::of(-9999, 1, 1).until(@time.PlainDate::of(9999, 1, 1)),
    content="P19998Y",
  )
}

///|
test "until and add_period" {
  let start = @time.PlainDate::of(2000, 1, 1)
  let p = start.until(@time.PlainDate::of(2024, 1, 1))
  inspect(start.add_period(p), content="2024-01-01")
  let p = start.until(@time.PlainDate::of(1970, 1, 1))
  inspect(start.add_period(p), content="1970-01-01")
  let p = start.until(@time.PlainDate::of(2000, 2, 15))
  inspect(start.add_period(p), content="2000-02-15")
  let p = start.until(@time.PlainDate::of(1999, 2, 15))
  inspect(start.add_period(p), content="1999-02-15")
}
