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
test "new" {
  // 1/2
  let a = @rational.new(1L, 2L)
  inspect(a, content="Some(1/2)")

  // 1/0
  let a = @rational.new(1L, 0L)
  inspect(a, content="None")
}

///|
test "addition" {
  let a = @rational.new(1L, 2L).unwrap()
  let b = @rational.new(1L, 3L).unwrap()
  let result = a + b
  inspect(result, content="5/6")
}

///|
test "subtraction" {
  let a = @rational.new(3L, 4L).unwrap()
  let b = @rational.new(1L, 4L).unwrap()
  let result = a - b
  inspect(result, content="1/2")
}

///|
test "multiplication" {
  let a = @rational.new(2L, 3L).unwrap()
  let b = @rational.new(3L, 4L).unwrap()
  let result = a * b
  inspect(result, content="1/2")
}

///|
test "division" {
  let a = @rational.new(1L, 2L).unwrap()
  let b = @rational.new(3L, 4L).unwrap()
  let result = a / b
  inspect(result, content="2/3")
}

///|
test "from_double overflow" {
  let result = try? @rational.from_double(10.0e200)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
  let result = try? @rational.from_double(-10.0e200)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
}

///|
test "from_double NaN" {
  let result = try? @rational.from_double(@double.not_a_number)
  assert_eq(
    result,
    Err(RationalError("Rational::from_double: cannot convert NaN")),
  )
}

///|
test "from_double overflow edge case" {
  let result = try? @rational.from_double(9_223_372_036_854_775_807.1)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
  let result = try? @rational.from_double(-9_223_372_036_854_775_807.1)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
}

///|
test "from_double infinity" {
  let result = try? @rational.from_double(@double.infinity)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
  let result = try? @rational.from_double(@double.neg_infinity)
  assert_eq(result, Err(RationalError("Rational::from_double: overflow")))
}

///|
test "from_double" {
  let a : Array[Double] = [
    0.5, 5, 29.97, -29.97, 63.5, 126.5, 127.0, 127.5, -63.5, -126.5, -127.0, -127.5,
  ]
  inspect(
    a.map(@rational.from_double),
    content="[1/2, 5, 2997/100, -2997/100, 127/2, 253/2, 127, 255/2, -127/2, -253/2, -127, -255/2]",
  )
}

///|
test "rational arbitrary" {
  let samples : Array[@rational.Rational64] = @quickcheck.samples(20)
  inspect(
    samples,
    content="[0, 0, 0, 0, 2/3, -2/5, 1, 1/6, 3/2, -1/2, 9/5, 5/3, -2, 7/12, -2/9, -8/15, 0, -5/6, 3/4, -3/7]",
  )
}

///|
test "rational equality" {
  let a = @rational.new(1L, 2L).unwrap()
  let b = @rational.new(2L, 4L).unwrap()
  let c = @rational.new(1L, 3L).unwrap()
  inspect(a == b, content="true")
  inspect(a == c, content="false")
  let a = @rational.new(-3L, -9223372036854775808L).unwrap()
  let b = @rational.new(-1L, -9223372036854775808L).unwrap()
  inspect(a == b, content="false")
  let x = @rational.new(-43L, 3689348814741910693L).unwrap()
  let y = @rational.new(5L, -43L).unwrap()
  inspect(x == y, content="false")
}
