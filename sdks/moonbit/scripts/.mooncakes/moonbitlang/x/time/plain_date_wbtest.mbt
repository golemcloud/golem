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
test "create_from_valid" {
  inspect(create_from_valid(2001, 2, 29), content="2001-02-28")
  inspect(create_from_valid(-2001, 2, 29), content="-2001-02-28")
}

///|
test "date_to_fixed_days" {
  inspect(date_to_fixed_days(0, 1, 1), content="0")
  inspect(date_to_fixed_days(1, 1, 1), content="366")
  inspect(date_to_fixed_days(2, 1, 1), content="731")
  inspect(date_to_fixed_days(-1, 1, 1), content="-365")
  inspect(date_to_fixed_days(-1970, 1, 1), content="-719527")
  inspect(date_to_fixed_days(1970, 1, 1), content="719528")
}

///|
test "fixed_days_to_year_ord" {
  inspect(fixed_days_to_year_ord(0L), content="(0, 1)")
  inspect(fixed_days_to_year_ord(1L), content="(0, 2)")
  inspect(fixed_days_to_year_ord(366L), content="(1, 1)")
  inspect(fixed_days_to_year_ord(731L), content="(2, 1)")
  inspect(fixed_days_to_year_ord(719528L), content="(1970, 1)")
  inspect(fixed_days_to_year_ord(-365L), content="(-1, 1)")
  inspect(fixed_days_to_year_ord(-730L), content="(-2, 1)")
  inspect(fixed_days_to_year_ord(-719527L), content="(-1970, 1)")
}
