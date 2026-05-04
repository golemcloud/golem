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
test "validate_time" {
  inspect(validate_time(1, 0, 0, 0), content="true")
  inspect(validate_time(0, 0, 0, 0), content="true")
  inspect(validate_time(23, 59, 59, 0), content="true")
  inspect(validate_time(24, 0, 0, 0), content="true")
  inspect(validate_time(1, 59, 0, 0), content="true")
  inspect(validate_time(1, 60, 0, 0), content="false")
  inspect(validate_time(1, 0, 59, 0), content="true")
  inspect(validate_time(1, 0, 60, 0), content="false")
  inspect(validate_time(1, 0, 0, 999_999_999), content="true")
  inspect(validate_time(1, 0, 0, 1_000_000_000), content="false")
}

///|
test "add_nanoseconds" {
  let time = PlainTime::of(0, 0, 00, 1000)
  inspect(time.add_nanoseconds(-1L), content="00:00:00.000000999")
  inspect(time.add_nanoseconds(0L), content="00:00:00.000001")
  inspect(time.add_nanoseconds(1L), content="00:00:00.000001001")
  inspect(time.add_nanoseconds(123456789L), content="00:00:00.123457789")
  inspect(time.add_nanoseconds(@int64.min_value), content="00:12:43.145225192")
  inspect(time.add_nanoseconds(@int64.max_value), content="23:47:16.854776807")
  inspect(time.add_nanoseconds(nanoseconds_per_day), content="00:00:00.000001")
}

///|
test "add_duration" {
  let time = PlainTime::of(12, 0, 0, 0)
  inspect(
    time.add_duration(
      Duration::of(hours=1L, minutes=30L, seconds=30L, nanoseconds=1_000_000L),
    ),
    content="13:30:30.001",
  )
  inspect(
    time.add_duration(
      Duration::of(minutes=-29L, seconds=-29L, nanoseconds=-999_000_000L),
    ),
    content="11:30:30.001",
  )
  inspect(time.add_duration(Duration::zero()), content="12:00:00")
}

///|
test "at_date" {
  let time = PlainTime::of(12, 0, 0, 0)
  let date = PlainDate::of(2000, 1, 1)
  inspect(time.at_date(date), content="2000-01-01T12:00:00")
}
