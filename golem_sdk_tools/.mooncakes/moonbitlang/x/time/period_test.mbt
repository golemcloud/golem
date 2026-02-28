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
    @time.Period::of(years=1000, months=24, days=365),
    content="P1000Y24M365D",
  )
}

///|
test "from_string" {
  inspect(@time.Period::from_string("P10Y2M3D"), content="P10Y2M3D")
  inspect(@time.Period::from_string("P-10Y-2M-3D"), content="P-10Y-2M-3D")
  inspect(@time.Period::from_string("P2M3D10Y"), content="P2M3D")
  assert_true((try? @time.Period::from_string("10Y")) is Err(_))
  assert_true((try? @time.Period::from_string("10Y0M")) is Err(_))
  assert_true((try? @time.Period::from_string("P10YT3H")) is Err(_))
  assert_true((try? @time.Period::from_string("P2.5Y")) is Err(_))
}

///|
test "getters" {
  let p = @time.Period::of(years=1, months=2, days=3)
  inspect(p.years(), content="1")
  inspect(p.months(), content="2")
  inspect(p.days(), content="3")
  inspect(p.to_total_months(), content="14")
}

///|
test "is_zero" {
  inspect(@time.Period::zero().is_zero(), content="true")
  inspect(@time.Period::of().is_zero(), content="true")
  inspect(@time.Period::of(years=1).is_zero(), content="false")
  inspect(@time.Period::of(months=1).is_zero(), content="false")
  inspect(@time.Period::of(days=1).is_zero(), content="false")
}

///|
test "is_neg" {
  inspect(@time.Period::of().is_neg(), content="false")
  inspect(@time.Period::of(years=1, months=1).is_neg(), content="false")
  inspect(@time.Period::of(years=-1, months=1).is_neg(), content="true")
}

///|
test "add_years" {
  let p = @time.Period::of(years=10)
  inspect(p.add_years(0), content="P10Y")
  inspect(p.add_years(1), content="P11Y")
  inspect(p.add_years(-1), content="P9Y")
}

///|
test "add_months" {
  let p = @time.Period::of(months=10)
  inspect(p.add_months(0), content="P10M")
  inspect(p.add_months(1), content="P11M")
  inspect(p.add_months(-1), content="P9M")
}

///|
test "add_weeks" {
  let p = @time.Period::of(days=10)
  inspect(p.add_weeks(0), content="P10D")
  inspect(p.add_weeks(1), content="P17D")
  inspect(p.add_weeks(-1), content="P3D")
}

///|
test "add_days" {
  let p = @time.Period::of(days=10)
  inspect(p.add_days(0), content="P10D")
  inspect(p.add_days(1), content="P11D")
  inspect(p.add_days(-1), content="P9D")
}

///|
test "with_years" {
  inspect(@time.Period::of(years=1).with_years(1), content="P1Y")
  inspect(@time.Period::of(years=1).with_years(2), content="P2Y")
  inspect(@time.Period::of(years=1).with_years(-1), content="P-1Y")
}

///|
test "with_months" {
  inspect(@time.Period::of(months=1).with_months(1), content="P1M")
  inspect(@time.Period::of(months=1).with_months(2), content="P2M")
  inspect(@time.Period::of(months=1).with_months(-1), content="P-1M")
}

///|
test "with_days" {
  inspect(@time.Period::of(days=1).with_days(1), content="P1D")
  inspect(@time.Period::of(days=1).with_days(2), content="P2D")
  inspect(@time.Period::of(days=1).with_days(-1), content="P-1D")
}

///|
test "op_add" {
  let p1 = @time.Period::of(years=1, months=2, days=3)
  let p2 = @time.Period::of(years=-1, months=-2, days=-3)
  let p3 = @time.Period::of(
    years=@int.max_value,
    months=@int.max_value,
    days=@int.max_value,
  )
  let p4 = @time.Period::of(
    years=@int.min_value,
    months=@int.min_value,
    days=@int.min_value,
  )
  inspect(p1.op_add(@time.Period::zero()), content="P1Y2M3D")
  inspect(p1.op_add(p2), content="P0D")
  inspect(p1.op_add(p1), content="P2Y4M6D")
  inspect(p2.op_add(p2), content="P-2Y-4M-6D")
  assert_true((try? p1.op_add(p3)) is Err(_))
  assert_true((try? p2.op_add(p4)) is Err(_))
}

///|
test "op_sub" {
  let p1 = @time.Period::of(years=1, months=2, days=3)
  let p2 = @time.Period::of(years=-1, months=-2, days=-3)
  let p3 = @time.Period::of(
    years=@int.max_value,
    months=@int.max_value,
    days=@int.max_value,
  )
  let p4 = @time.Period::of(
    years=@int.min_value,
    months=@int.min_value,
    days=@int.min_value,
  )
  inspect(p1.op_sub(p1), content="P0D")
  inspect(p1.op_sub(p2), content="P2Y4M6D")
  inspect(p2.op_sub(p1), content="P-2Y-4M-6D")
  assert_true((try? p3.op_sub(p2)) is Err(_))
  assert_true((try? p4.op_sub(p1)) is Err(_))
}

///|
test "multiply" {
  let p = @time.Period::of(years=1, months=2, days=3)
  inspect(p.multiply(0), content="P0D")
  inspect(p.multiply(1), content="P1Y2M3D")
  inspect(p.multiply(-1), content="P-1Y-2M-3D")
  inspect(p.multiply(2), content="P2Y4M6D")
  assert_true((try? p.multiply(@int.max_value)) is Err(_))
  assert_true((try? p.multiply(@int.min_value)) is Err(_))
}

///|
test "negated" {
  let p = @time.Period::of(years=1, months=1, days=1)
  inspect(p.negated(), content="P-1Y-1M-1D")
  inspect(p.negated().negated() == p, content="true")
  let p = @time.Period::of(
    years=@int.max_value,
    months=@int.max_value,
    days=@int.max_value,
  )
  inspect(p.negated(), content="P-2147483647Y-2147483647M-2147483647D")
  inspect(p.negated().negated() == p, content="true")
  let p = @time.Period::of(
    years=@int.min_value,
    months=@int.min_value,
    days=@int.min_value,
  )
  assert_true((try? p.negated()) is Err(_))
}
