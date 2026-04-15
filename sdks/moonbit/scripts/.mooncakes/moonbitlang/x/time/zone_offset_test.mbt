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
  inspect(@time.ZoneOffset::of(), content="Z")
  inspect(@time.ZoneOffset::of(hours=8), content="+08:00")
  inspect(@time.ZoneOffset::of(hours=-8), content="-08:00")
  inspect(@time.ZoneOffset::of(hours=18), content="+18:00")
  inspect(@time.ZoneOffset::of(hours=-18), content="-18:00")
  inspect(
    @time.ZoneOffset::of(hours=1, minutes=30, seconds=30),
    content="+01:30:30",
  )
  assert_true((try? @time.ZoneOffset::of(hours=19)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(hours=-19)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(minutes=60)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(minutes=-60)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(seconds=60)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(seconds=-60)) is Err(_))
  assert_true(
    (try? @time.ZoneOffset::of(hours=1, minutes=-30, seconds=-30)) is Err(_),
  )
  assert_true(
    (try? @time.ZoneOffset::of(hours=-1, minutes=30, seconds=30)) is Err(_),
  )
  assert_true((try? @time.ZoneOffset::of(minutes=-30, seconds=30)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(minutes=30, seconds=-30)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(hours=18, minutes=1)) is Err(_))
  assert_true((try? @time.ZoneOffset::of(hours=-18, seconds=1)) is Err(_))
}

///|
test "from_seconds" {
  inspect(@time.ZoneOffset::from_seconds(8 * 60 * 60), content="+08:00")
  inspect(@time.ZoneOffset::from_seconds(0), content="Z")
  inspect(@time.ZoneOffset::from_seconds(18 * 60 * 60), content="+18:00")
  inspect(@time.ZoneOffset::from_seconds(-18 * 60 * 60), content="-18:00")
  assert_true((try? @time.ZoneOffset::from_seconds(19 * 60 * 60)) is Err(_))
  assert_true((try? @time.ZoneOffset::from_seconds(-19 * 60 * 60)) is Err(_))
}

///|
test "utc" {
  inspect(@time.utc_offset, content="Z")
  inspect(@time.utc_offset.id(), content="Z")
  inspect(@time.utc_offset.seconds(), content="0")
  inspect(@time.utc_offset.is_dst(), content="false")
}

///|
test "compare" {
  let o1 = @time.ZoneOffset::from_seconds(10)
  let o2 = @time.ZoneOffset::from_seconds(-10)
  inspect(o1 > o2, content="false")
  inspect(o1 == o2, content="false")
  inspect(o1 < o2, content="true")
  inspect(o1 == o1, content="true")
}
