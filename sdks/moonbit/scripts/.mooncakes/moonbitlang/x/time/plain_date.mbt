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
/// A date without a time zone in the ISO 8601 calendar system.
struct PlainDate {
  year : Int
  month : Int
  day : Int
}

///|
/// Creates a PlainDate from the year, month and day.
pub fn PlainDate::of(
  year : Int,
  month : Int,
  day : Int,
) -> PlainDate raise Error {
  if not(validate_ymd(year, month, day)) {
    fail(invalid_date_err)
  }
  PlainDate::{ year, month, day }
}

///|
/// Creates a PlainDate from year and ordinal day.
pub fn PlainDate::from_year_ord(
  year : Int,
  ordinal : Int,
) -> PlainDate raise Error {
  let valid = validate_year(year) && validate_ordinal(year, ordinal)
  if not(valid) {
    fail(invalid_date_err)
  }
  let mut month = 1
  let mut day = ordinal
  while calc_days_in_month(year, month) < day {
    day -= calc_days_in_month(year, month)
    month += 1
  }
  PlainDate::{ year, month, day }
}

///|
/// Creates a date from the days count since unix epoch.
pub fn PlainDate::from_unix_day(unix_day : Int64) -> PlainDate raise Error {
  let (year, ord) = fixed_days_to_year_ord(
    unix_day + days_zero_year_to_unix_epoch,
  )
  PlainDate::from_year_ord(year, ord)
}

///|
/// Returns the days count since unix epoch of this date.
pub fn PlainDate::to_unix_day(self : PlainDate) -> Int64 {
  let days = date_to_fixed_days(self.year, self.month, self.day)
  days - days_zero_year_to_unix_epoch
}

///|
/// Creates a PlainDate from a string, like "2008-08-08".
pub fn PlainDate::from_string(str : String) -> PlainDate raise Error {
  lexmatch str with longest {
    (
      ("-?[0-9]{4}" as year_str)
      "-"
      ("[0-9]{2}" as month_str)
      "-"
      ("[0-9]{2}" as day_str)
    ) => {
      let year = @strconv.parse_int(year_str)
      let month = @strconv.parse_int(month_str)
      let day = @strconv.parse_int(day_str)
      PlainDate::of(year, month, day)
    }
    _ => fail(invalid_date_err)
  }
}

///|
/// Returns a string representing the date.
pub fn PlainDate::to_string(self : PlainDate) -> String {
  let buf = StringBuilder::new(size_hint=11)
  if self.year < 0 {
    buf.write_char('-')
  }
  buf.write_string(add_prefix_zero(self.year.abs().to_string(), 4))
  buf.write_char('-')
  buf.write_string(add_prefix_zero(self.month.to_string(), 2))
  buf.write_char('-')
  buf.write_string(add_prefix_zero(self.day.to_string(), 2))
  buf.to_string()
}

///|
pub impl Show for PlainDate with output(self : PlainDate, logger : &Logger) {
  logger.write_string(self.to_string())
}

///|
pub impl Eq for PlainDate with equal(self : PlainDate, other : PlainDate) -> Bool {
  self.compare(other) == 0
}

///|
pub impl Compare for PlainDate with compare(self : PlainDate, other : PlainDate) -> Int {
  if self.year > other.year {
    1
  } else if self.year < other.year {
    -1
  } else if self.month > other.month {
    1
  } else if self.month < other.month {
    -1
  } else if self.day > other.day {
    1
  } else if self.day < other.day {
    -1
  } else {
    0
  }
}

///|
/// Returns the era of this date.
pub fn PlainDate::era(self : PlainDate) -> String {
  if self.year >= 1 {
    "CE"
  } else {
    "BCE"
  }
}

///|
/// Returns the year of the era of this date.
pub fn PlainDate::era_year(self : PlainDate) -> Int {
  if self.year <= 0 {
    self.year.abs() + 1
  } else {
    self.year
  }
}

///|
/// Returns the number of years relative to a calendar-specific epoch.
pub fn PlainDate::year(self : PlainDate) -> Int {
  self.year
}

///|
/// Returns the ordinal number of month in the current year.
pub fn PlainDate::month(self : PlainDate) -> Int {
  self.month
}

///|
/// Returns the day of the month.
pub fn PlainDate::day(self : PlainDate) -> Int {
  self.day
}

///|
/// Returns the weekday.
pub fn PlainDate::weekday(self : PlainDate) -> Weekday {
  // Zeller's congruence
  // https://en.wikipedia.org/wiki/Zeller%27s_congruence
  let mut y = self.year
  let mut m = self.month
  let q = self.day
  if m < 3 {
    m += 12
    y -= 1
  }
  let k = y % 100
  let j = y / 100
  let h = (q + 13 * (m + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7
  let week_day = (h + 5) % 7 + 1
  Weekday::new(week_day).unwrap()
}

///|
/// Returns the ordinal day of the year.
pub fn PlainDate::ordinal(self : PlainDate) -> Int {
  let days = date_to_fixed_days(self.year, self.month, self.day)
  let (_, ordinal) = fixed_days_to_year_ord(days)
  ordinal
}

///|
/// Returns the number of days in the week.
pub fn PlainDate::days_in_week(_self : PlainDate) -> Int {
  7
}

///|
/// Returns the number of days in the month.
pub fn PlainDate::days_in_month(self : PlainDate) -> Int {
  let { year, month, .. } = self
  calc_days_in_month(year, month)
}

///|
/// Returns the number of days in the year.
pub fn PlainDate::days_in_year(self : PlainDate) -> Int {
  calc_days_in_year(self.year)
}

///|
/// Returns the number of months in the year.
pub fn PlainDate::months_in_year(_self : PlainDate) -> Int {
  12
}

///|
/// Checks if the date is in a leap year.
pub fn PlainDate::in_leap_year(self : PlainDate) -> Bool {
  check_leap_year(self.year)
}

///|
/// Returns a new date with the specified year.
pub fn PlainDate::with_year(
  self : PlainDate,
  year : Int,
) -> PlainDate raise Error {
  if self.year == year {
    return self
  }
  if validate_year(year) {
    let new_date = create_from_valid(year, self.month, self.day)
    new_date
  } else {
    fail(invalid_date_err)
  }
}

///|
/// Returns a new date with the specified month.
pub fn PlainDate::with_month(
  self : PlainDate,
  month : Int,
) -> PlainDate raise Error {
  if self.month == month {
    return self
  }
  if validate_month(month) {
    let new_date = create_from_valid(self.year, month, self.day)
    new_date
  } else {
    fail(invalid_date_err)
  }
}

///|
/// Returns a new date with the specified day of month.
pub fn PlainDate::with_day(
  self : PlainDate,
  day : Int,
) -> PlainDate raise Error {
  if self.day == day {
    return self
  }
  if validate_day(self.year, self.month, day) {
    let new_date = create_from_valid(self.year, self.month, day)
    new_date
  } else {
    fail(invalid_date_err)
  }
}

///|
/// Returns a new date with the specified ordinal day of year.
pub fn PlainDate::with_ordinal(
  self : PlainDate,
  ordinal : Int,
) -> PlainDate raise Error {
  if self.ordinal() == ordinal {
    return self
  }
  PlainDate::from_year_ord(self.year, ordinal)
}

///|
/// Adds specified years to this date, and returns a new date.
pub fn PlainDate::add_years(
  self : PlainDate,
  years : Int64,
) -> PlainDate raise Error {
  if years == 0L {
    return self
  }
  let new_year = checked_add_int64(self.year.to_int64(), years).to_int()
  if validate_year(new_year) {
    let new_date = create_from_valid(new_year, self.month, self.day)
    new_date
  } else {
    fail(invalid_date_err)
  }
}

///|
/// Adds specified months to this date, and returns a new date.
pub fn PlainDate::add_months(
  self : PlainDate,
  months : Int64,
) -> PlainDate raise Error {
  if months == 0L {
    return self
  }
  let total_months = self.year.to_int64() * 12L + (self.month.to_int64() - 1L)
  let new_total_months = checked_add_int64(total_months, months)
  let mut new_year = (new_total_months / 12L).to_int()
  let mut new_month = (new_total_months % 12L + 1L).to_int()
  if new_month < 0 {
    new_year -= 1
    new_month += 12
  }
  if validate_year(new_year) {
    create_from_valid(new_year, new_month, self.day)
  } else {
    fail(invalid_date_err)
  }
}

///|
/// Adds specified weeks to this date, and returns a new date.
pub fn PlainDate::add_weeks(
  self : PlainDate,
  weeks : Int64,
) -> PlainDate raise Error {
  if weeks == 0L {
    return self
  }
  let days_to_add = checked_mul_int64(weeks, 7L)
  self.add_days(days_to_add)
}

///|
/// Adds specified days to this date, and returns a new date.
pub fn PlainDate::add_days(
  self : PlainDate,
  days : Int64,
) -> PlainDate raise Error {
  if days == 0L {
    return self
  }
  let fixed_days = date_to_fixed_days(self.year, self.month, self.day)
  let new_days = checked_add_int64(fixed_days, days)
  let (year, ordinal) = fixed_days_to_year_ord(new_days)
  PlainDate::from_year_ord(year, ordinal)
}

///|
/// Adds a period to this date, and returns a new date.
pub fn PlainDate::add_period(
  self : PlainDate,
  period : Period,
) -> PlainDate raise Error {
  let mut d = self
  d = d.add_months(period.to_total_months()).add_days(period.days().to_int64())
  d
}

///|
/// Returns the period between this date and another date.
pub fn PlainDate::until(
  self : PlainDate,
  end : PlainDate,
) -> Period raise Error {
  let start_months = self.year.to_int64() * 12L + self.month.to_int64() - 1L
  let end_months = end.year.to_int64() * 12L + end.month.to_int64() - 1L
  let mut total_months = end_months - start_months
  let mut days = end.day - self.day
  if total_months > 0L && days < 0 {
    total_months -= 1L
    let d = self.add_months(total_months)
    let end_days = date_to_fixed_days(end.year, end.month, end.day)
    days = (end_days - date_to_fixed_days(d.year, d.month, d.day)).to_int()
  } else if total_months < 0L && days > 0 {
    total_months += 1L
    days -= end.days_in_month()
  }
  let years = (total_months / 12L).to_int()
  let months = (total_months % 12L).to_int()
  Period::of(years~, months~, days~)
}

// *****************************
// * internal helper functions *
// *****************************

///|
let min_year = -9999

///|
let max_year = 9999

///|
let min_month = 1

///|
let max_month = 12

///|
let min_day = 1

///|
let max_day = 31

///|
let min_ordinal = 1

///|
let max_ordinal = 366

///|
let days_per_400_years : Int64 = 400L * 365L + 97L

///|
let days_zero_year_to_unix_epoch : Int64 = 5L * days_per_400_years -
  (30L * 365L + 7L)

///|
fn create_from_valid(year : Int, month : Int, day : Int) -> PlainDate {
  let mut d = day
  let max_day = calc_days_in_month(year, month)
  if d > max_day {
    d = max_day
  }
  PlainDate::{ year, month, day: d }
}

///|
fn check_leap_year(year : Int) -> Bool {
  (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

///|
fn calc_days_in_month(year : Int, month : Int) -> Int {
  let leap = check_leap_year(year)
  match month {
    2 => if leap { 29 } else { 28 }
    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31
    _ => 30
  }
}

///|
fn calc_days_in_year(year : Int) -> Int {
  if check_leap_year(year) {
    366
  } else {
    365
  }
}

///|
fn date_to_fixed_days(year : Int, month : Int, day : Int) -> Int64 {
  let mut days = 0L
  // year days
  let y = year.to_int64()
  // non leap days
  days += 365L * y
  // leap days
  if y > 0L {
    days += (y + 3L) / 4L - (y + 99L) / 100L + (y + 399L) / 400L
  } else {
    days += y / 4L - y / 100L + y / 400L
  }
  // month days
  days += (367L * month.to_int64() - 362L) / 12L
  if month > 2 {
    days += if check_leap_year(year) { -1L } else { -2L }
  }
  days += day.to_int64() - 1L
  days
}

///|
fn fixed_days_to_year_ord(days : Int64) -> (Int, Int) {
  let mut d0 = days
  // adjust to positive days for calculation
  let mut year_offset = 0
  if d0 < 0L {
    let cycles = days / days_per_400_years - 1L
    year_offset = cycles.to_int() * 400
    d0 -= cycles * days_per_400_years
  }
  // calculate year
  let n400 = d0 / days_per_400_years
  let cycle = d0 % days_per_400_years
  let (y, ord) = cycle_to_year_ord(cycle.to_int())
  let year = y + n400.to_int() * 400 + year_offset
  (year, ord)
}

///|
let year_deltas : ReadOnlyArray[Int] = [
  0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 7, 7,
  7, 7, 8, 8, 8, 8, 9, 9, 9, 9, 10, 10, 10, 10, 11, 11, 11, 11, 12, 12, 12, 12, 13,
  13, 13, 13, 14, 14, 14, 14, 15, 15, 15, 15, 16, 16, 16, 16, 17, 17, 17, 17, 18,
  18, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20, 21, 21, 21, 21, 22, 22, 22, 22, 23,
  23, 23, 23, 24, 24, 24, 24, 25, 25, 25, // 100
   25, 25, 25, 25, 25, 26, 26, 26, 26, 27, 27, 27, 27, 28, 28, 28, 28, 29, 29, 29,
  29, 30, 30, 30, 30, 31, 31, 31, 31, 32, 32, 32, 32, 33, 33, 33, 33, 34, 34, 34,
  34, 35, 35, 35, 35, 36, 36, 36, 36, 37, 37, 37, 37, 38, 38, 38, 38, 39, 39, 39,
  39, 40, 40, 40, 40, 41, 41, 41, 41, 42, 42, 42, 42, 43, 43, 43, 43, 44, 44, 44,
  44, 45, 45, 45, 45, 46, 46, 46, 46, 47, 47, 47, 47, 48, 48, 48, 48, 49, 49, 49, // 200
   49, 49, 49, 49, 49, 50, 50, 50, 50, 51, 51, 51, 51, 52, 52, 52, 52, 53, 53, 53,
  53, 54, 54, 54, 54, 55, 55, 55, 55, 56, 56, 56, 56, 57, 57, 57, 57, 58, 58, 58,
  58, 59, 59, 59, 59, 60, 60, 60, 60, 61, 61, 61, 61, 62, 62, 62, 62, 63, 63, 63,
  63, 64, 64, 64, 64, 65, 65, 65, 65, 66, 66, 66, 66, 67, 67, 67, 67, 68, 68, 68,
  68, 69, 69, 69, 69, 70, 70, 70, 70, 71, 71, 71, 71, 72, 72, 72, 72, 73, 73, 73, // 300
   73, 73, 73, 73, 73, 74, 74, 74, 74, 75, 75, 75, 75, 76, 76, 76, 76, 77, 77, 77,
  77, 78, 78, 78, 78, 79, 79, 79, 79, 80, 80, 80, 80, 81, 81, 81, 81, 82, 82, 82,
  82, 83, 83, 83, 83, 84, 84, 84, 84, 85, 85, 85, 85, 86, 86, 86, 86, 87, 87, 87,
  87, 88, 88, 88, 88, 89, 89, 89, 89, 90, 90, 90, 90, 91, 91, 91, 91, 92, 92, 92,
  92, 93, 93, 93, 93, 94, 94, 94, 94, 95, 95, 95, 95, 96, 96, 96, 96, 97, 97, 97,
  97, // 400+1
]

///|
fn cycle_to_year_ord(cycle : Int) -> (Int, Int) {
  let mut year = cycle / 365
  let mut ordinal0 = cycle % 365
  let delta = year_deltas[year]
  if ordinal0 < delta {
    year -= 1
    ordinal0 += 365 - year_deltas[year]
  } else {
    ordinal0 -= delta
  }
  (year, ordinal0 + 1)
}

///|
fn validate_ymd(year : Int, month : Int, day : Int) -> Bool {
  validate_year(year) && validate_month(month) && validate_day(year, month, day)
}

///|
fn validate_year(year : Int) -> Bool {
  year >= min_year && year <= max_year
}

///|
fn validate_month(month : Int) -> Bool {
  month >= min_month && month <= max_month
}

///|
fn validate_day(year : Int, month : Int, day : Int) -> Bool {
  day >= min_day && day <= max_day && day <= calc_days_in_month(year, month)
}

///|
fn validate_ordinal(year : Int, ordinal : Int) -> Bool {
  ordinal >= min_ordinal &&
  ordinal <= max_ordinal &&
  ordinal <= calc_days_in_year(year)
}
