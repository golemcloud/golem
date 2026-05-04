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
  inspect(
    Duration::of(hours=1L, minutes=2L, seconds=3L, nanoseconds=456789L),
    content="PT1H2M3.000456789S",
  )
  inspect(Duration::of(seconds=-1L, nanoseconds=1000L), content="PT-0.999999S")
  inspect(
    Duration::of(seconds=-1L, nanoseconds=999_999_999L),
    content="PT-0.000000001S",
  )
  inspect(
    Duration::of(hours=-1L, seconds=-10L, nanoseconds=2000000L),
    content="PT-1H-9.998S",
  )
  inspect(
    Duration::of(nanoseconds=-20L * nanoseconds_per_second - 2L),
    content="PT-20.000000002S",
  )
  inspect(Duration::of(nanoseconds=-200L), content="PT-0.0000002S")
}

///|
test "add_hours" {
  inspect(Duration::zero().add_hours(0L), content="PT0S")
  inspect(Duration::zero().add_hours(1L), content="PT1H")
  inspect(Duration::zero().add_hours(-1L), content="PT-1H")
  inspect(
    Duration::zero().add_hours(@int64.max_value / seconds_per_hour),
    content="PT2562047788015215H",
  )
  inspect(
    Duration::zero().add_hours(@int64.min_value / seconds_per_hour),
    content="PT-2562047788015215H",
  )
  assert_true((try? Duration::zero().add_hours(@int64.max_value)) is Err(_))
  assert_true((try? Duration::zero().add_hours(@int64.min_value)) is Err(_))
}

///|
test "add_minutes" {
  inspect(Duration::zero().add_minutes(0L), content="PT0S")
  inspect(Duration::zero().add_minutes(1L), content="PT1M")
  inspect(Duration::zero().add_minutes(-1L), content="PT-1M")
  inspect(
    Duration::zero().add_minutes(@int64.max_value / seconds_per_minute),
    content="PT2562047788015215H30M",
  )
  inspect(
    Duration::zero().add_minutes(@int64.min_value / seconds_per_minute),
    content="PT-2562047788015215H-30M",
  )
  assert_true((try? Duration::zero().add_minutes(@int64.max_value)) is Err(_))
  assert_true((try? Duration::zero().add_minutes(@int64.min_value)) is Err(_))
}

///|
test "add_nanoseconds" {
  inspect(Duration::zero().add_nanoseconds(0L), content="PT0S")
  inspect(Duration::zero().add_nanoseconds(1L), content="PT0.000000001S")
  inspect(Duration::zero().add_nanoseconds(-1L), content="PT-0.000000001S")
  inspect(
    Duration::zero().add_nanoseconds(nanoseconds_per_second),
    content="PT1S",
  )
  inspect(
    Duration::zero().add_nanoseconds(-nanoseconds_per_second),
    content="PT-1S",
  )
  inspect(
    Duration::zero().add_nanoseconds(nanoseconds_per_minute),
    content="PT1M",
  )
  inspect(
    Duration::zero().add_nanoseconds(-nanoseconds_per_minute),
    content="PT-1M",
  )
  inspect(
    Duration::zero().add_nanoseconds(nanoseconds_per_hour),
    content="PT1H",
  )
  inspect(
    Duration::zero().add_nanoseconds(-nanoseconds_per_hour),
    content="PT-1H",
  )
  inspect(
    Duration::zero().add_nanoseconds(@int64.max_value),
    content="PT2562047H47M16.854775807S",
  )
  inspect(
    Duration::zero().add_nanoseconds(@int64.min_value),
    content="PT-2562047H-47M-16.854775808S",
  )
  assert_true(
    (try? Duration::of(seconds=@int64.max_value).add_nanoseconds(
      @int64.max_value,
    ))
    is Err(_),
  )
  assert_true(
    (try? Duration::of(seconds=@int64.min_value).add_nanoseconds(
      @int64.min_value,
    ))
    is Err(_),
  )
}

///|
test "op_add" {
  let p1 = Duration::of(hours=1L, minutes=1L, seconds=1L, nanoseconds=1000L)
  let p2 = Duration::of(hours=-1L, minutes=-1L, seconds=-1L, nanoseconds=-1000L)
  let p3 = Duration::of(
    seconds=@int64.max_value,
    nanoseconds=max_nanosecond.to_int64(),
  )
  let p4 = Duration::of(seconds=@int64.min_value)
  inspect(p1.op_add(Duration::zero()), content="PT1H1M1.000001S")
  inspect(p1.op_add(p2), content="PT0S")
  inspect(p1.op_add(p1), content="PT2H2M2.000002S")
  inspect(p2.op_add(p2), content="PT-2H-2M-2.000002S")
  assert_true((try? p1.op_add(p3)) is Err(_))
  assert_true((try? p2.op_add(p4)) is Err(_))
}
