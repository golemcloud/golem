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
test "add_nanoseconds!" {
  let d = PlainDateTime::of(0, 1, 1)
  inspect(d.add_nanoseconds(0L), content="0000-01-01T00:00:00")
  inspect(d.add_nanoseconds(1L), content="0000-01-01T00:00:00.000000001")
  inspect(
    d.add_nanoseconds(nanoseconds_per_second),
    content="0000-01-01T00:00:01",
  )
  inspect(
    d.add_nanoseconds(nanoseconds_per_minute),
    content="0000-01-01T00:01:00",
  )
  inspect(
    d.add_nanoseconds(nanoseconds_per_hour),
    content="0000-01-01T01:00:00",
  )
  inspect(d.add_nanoseconds(nanoseconds_per_day), content="0000-01-02T00:00:00")
  inspect(
    d.add_nanoseconds(nanoseconds_per_day * 366L),
    content="0001-01-01T00:00:00",
  )
}
