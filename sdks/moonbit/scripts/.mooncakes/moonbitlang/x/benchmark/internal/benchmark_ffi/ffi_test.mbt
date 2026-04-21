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
/// We cannot actually test timestamp-getting methods, so this file only includes smoke tests for now.
test "instant_elapsed_as_secs_f64() returns a reasonable value" {
  let i = instant_now()
  let elapsed = instant_elapsed_as_secs_f64(i)
  inspect(elapsed >= 0.0, content="true")
}
