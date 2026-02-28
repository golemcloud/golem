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
test "from_string" {
  inspect(@time.Duration::from_string("PT0S"), content="PT0S")
  inspect(
    @time.Duration::from_string("PT1H2M3.456789S"),
    content="PT1H2M3.456789S",
  )
  inspect(
    @time.Duration::from_string("PT-1H-2M-3.456789S"),
    content="PT-1H-2M-3.456789S",
  )
  inspect(@time.Duration::from_string("PT0.45S"), content="PT0.45S")
  assert_true((try? @time.Duration::from_string("PT0.000.45S")) is Err(_))
  assert_true((try? @time.Duration::from_string("P1Y2M3DT4H5M6S")) is Err(_))
}

///|
test "getters" {
  let d = @time.Duration::of(hours=1L, minutes=2L, seconds=3L, nanoseconds=400L)
  inspect(d.seconds(), content="3723")
  inspect(d.nanoseconds(), content="400")
}

///|
test "is_zero" {
  inspect(@time.Duration::zero().is_zero(), content="true")
  inspect(@time.Duration::of().is_zero(), content="true")
  inspect(@time.Duration::of(hours=1L).is_zero(), content="false")
  inspect(@time.Duration::of(minutes=1L).is_zero(), content="false")
  inspect(@time.Duration::of(seconds=1L).is_zero(), content="false")
  inspect(@time.Duration::of(nanoseconds=1L).is_zero(), content="false")
}

///|
test "is_neg" {
  inspect(@time.Duration::zero().is_neg(), content="false")
  inspect(@time.Duration::of(seconds=1L).is_neg(), content="false")
  inspect(@time.Duration::of(seconds=-1L).is_neg(), content="true")
  inspect(
    @time.Duration::of(seconds=-1L, nanoseconds=1L).is_neg(),
    content="true",
  )
  inspect(@time.Duration::of(nanoseconds=1L).is_neg(), content="false")
}

///|
test "add_seconds" {
  inspect(@time.Duration::zero().add_seconds(0L), content="PT0S")
  inspect(@time.Duration::zero().add_seconds(1L), content="PT1S")
  inspect(@time.Duration::zero().add_seconds(-1L), content="PT-1S")
  inspect(
    @time.Duration::zero().add_seconds(@int64.max_value),
    content="PT2562047788015215H30M7S",
  )
  inspect(
    @time.Duration::zero().add_seconds(@int64.min_value),
    content="PT-2562047788015215H-30M-8S",
  )
  assert_true(
    (try? @time.Duration::of(seconds=1L).add_seconds(@int64.max_value))
    is Err(_),
  )
  assert_true(
    (try? @time.Duration::of(seconds=-1L).add_seconds(@int64.min_value))
    is Err(_),
  )
}

///|
test "with_seconds" {
  let d = @time.Duration::of(seconds=20L)
  inspect(d.with_seconds(20L), content="PT20S")
  inspect(d.with_seconds(30L), content="PT30S")
}

///|
test "with_nanoseconds" {
  let d = @time.Duration::of(nanoseconds=1000L)
  inspect(d.with_nanoseconds(1000), content="PT0.000001S")
  inspect(d.with_nanoseconds(2000), content="PT0.000002S")
  assert_true((try? d.with_nanoseconds(1_000_000_000)) is Err(_))
}
