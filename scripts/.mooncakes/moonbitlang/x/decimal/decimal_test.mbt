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
test "decimal_creation" {
  // Test from_string
  let d1 = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse '123.45'")
  }
  inspect(d1.coefficient(), content="12345")
  inspect(d1.scale(), content="2")
  let d2 = match @decimal.Decimal::from_string("0.001") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d2.coefficient(), content="1")
  inspect(d2.scale(), content="3")
  let d3 = match @decimal.Decimal::from_string("-42.0") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d3.coefficient(), content="-42")
  inspect(d3.scale(), content="0")
  let d4 = match @decimal.Decimal::from_string("0") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d4.coefficient(), content="0")
  inspect(d4.scale(), content="0")

  // Test from_int
  let d5 = @decimal.Decimal::from_int(42)
  inspect(d5.coefficient(), content="42")
  inspect(d5.scale(), content="0")

  // Test from_bigint
  let d6 = @decimal.Decimal::from_bigint(12345678901234567890N)
  inspect(d6.coefficient(), content="12345678901234567890")
  inspect(d6.scale(), content="0")

  // Test from_double
  let d7 = match @decimal.Decimal::from_double(3.14159, 5) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d7.to_string(), content="3.14159")

  // Test zero, one, neg_one
  let zero = @decimal.zero()
  inspect(zero.coefficient(), content="0")
  inspect(zero.scale(), content="0")
  let one = @decimal.one()
  inspect(one.coefficient(), content="1")
  inspect(one.scale(), content="0")
  let neg_one = @decimal.neg_one()
  inspect(neg_one.coefficient(), content="-1")
  inspect(neg_one.scale(), content="0")
}

///|
test "decimal_string_conversion" {
  // Test to_string
  let d1 = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d1.to_string(), content="123.45")
  let d2 = match @decimal.Decimal::from_string("0.001") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d2.to_string(), content="0.001")
  let d3 = match @decimal.Decimal::from_string("-42.0") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d3.to_string(), content="-42")
  let d4 = match @decimal.Decimal::from_string("0") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d4.to_string(), content="0")
  let d5 = match @decimal.Decimal::from_string("1000.0") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d5.to_string(), content="1000")
  let d6 = match @decimal.Decimal::from_string("0.0001") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d6.to_string(), content="0.0001")

  // Test edge cases
  let d7 = match @decimal.Decimal::new(12345N, 2) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d7.to_string(), content="123.45")
  let d8 = match @decimal.Decimal::new(1N, 3) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d8.to_string(), content="0.001")
  let d9 = match @decimal.Decimal::new(-1N, 3) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d9.to_string(), content="-0.001")
}

///|
test "decimal_arithmetic" {
  // Test addition
  let a = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let b = match @decimal.Decimal::from_string("67.89") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let sum = a + b
  inspect(sum.to_string(), content="191.34")

  // Test subtraction
  let diff = a - b
  inspect(diff.to_string(), content="55.56")

  // Test multiplication
  let product = a * b
  inspect(product.to_string(), content="8381.0205")

  // Test division
  let quotient = a / b
  inspect(quotient.to_string(), content="1.8183826778612461334511710119")

  // Test with different scales
  let c = match @decimal.Decimal::from_string("1.5") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let d = match @decimal.Decimal::from_string("2.25") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let sum2 = c + d
  inspect(sum2.to_string(), content="3.75")
  let product2 = c * d
  inspect(product2.to_string(), content="3.375")
}

///|
test "decimal_comparison" {
  let a = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let b = match @decimal.Decimal::from_string("67.89") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let c = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }

  // Test equality
  assert_true(a == c)
  assert_false(a == b)

  // Test comparison
  assert_true(a > b)
  assert_true(b < a)
  assert_true(a >= c)
  assert_true(a <= c)

  // Test with different scales
  let d = match @decimal.Decimal::from_string("123.450") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  assert_true(a == d)
  let e = match @decimal.Decimal::from_string("123.4") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  assert_true(a > e)
  assert_true(e < a)
}

///|
test "decimal_rounding" {
  let d = match @decimal.Decimal::from_string("3.14159") {
    Some(d) => d
    None => fail("Failed to parse")
  }

  // Test round
  let rounded = match d.round(2) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(rounded.to_string(), content="3.14")
  let rounded2 = match d.round(3) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(rounded2.to_string(), content="3.142")

  // Test truncate
  let truncated = match d.truncate(2) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(truncated.to_string(), content="3.14")
  let truncated2 = match d.truncate(3) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(truncated2.to_string(), content="3.141")

  // Test scale_to
  let scaled = match d.scale_to(4) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(scaled.to_string(), content="3.1415")
  let scaled2 = match d.scale_to(6) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(scaled2.to_string(), content="3.141590")
}

///|
test "decimal_properties" {
  let d1 = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let d2 = match @decimal.Decimal::from_string("-67.89") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let zero = @decimal.zero()

  // Test is_zero
  assert_false(d1.is_zero())
  assert_false(d2.is_zero())
  assert_true(zero.is_zero())

  // Test is_positive
  assert_true(d1.is_positive())
  assert_false(d2.is_positive())
  assert_false(zero.is_positive())

  // Test is_negative
  assert_false(d1.is_negative())
  assert_true(d2.is_negative())
  assert_false(zero.is_negative())

  // Test signum
  inspect(d1.signum(), content="1")
  inspect(d2.signum(), content="-1")
  inspect(zero.signum(), content="0")

  // Test abs
  let abs_d2 = d2.abs()
  inspect(abs_d2.to_string(), content="67.89")

  // Test neg
  let neg_d1 = d1.neg()
  inspect(neg_d1.to_string(), content="-123.45")
}

///|
test "decimal_conversion" {
  let d1 = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let d2 = match @decimal.Decimal::from_string("123.99") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let d3 = match @decimal.Decimal::from_string("3.14159") {
    Some(d) => d
    None => fail("Failed to parse")
  }

  // Test to_int
  let int1 = d1.to_int()
  inspect(int1, content="Some(123)")
  let int2 = d2.to_int()
  inspect(int2, content="Some(123)")

  // Test to_bigint
  let bigint1 = d1.to_bigint()
  inspect(bigint1, content="123")
  let bigint2 = d2.to_bigint()
  inspect(bigint2, content="123")

  // Test to_double
  let double1 = d1.to_double()
  inspect(double1, content="123.45")
  let double3 = d3.to_double()
  inspect(double3, content="3.14159")
}

///|
test "decimal_edge_cases" {
  // Test very small numbers
  let tiny = match
    @decimal.Decimal::from_string("0.0000000000000000000000000001") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(tiny.to_string(), content="0.0000000000000000000000000001")

  // Test very large numbers
  let large = match
    @decimal.Decimal::from_string("123456789012345678901234567890.123456789") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(large.to_string(), content="123456789012345678901234567890.123456789")

  // Test negative numbers
  let neg = match @decimal.Decimal::from_string("-123.456") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(neg.to_string(), content="-123.456")

  // Test zero with different scales
  let zero1 = match @decimal.Decimal::new(0N, 0) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let zero2 = match @decimal.Decimal::new(0N, 5) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  assert_true(zero1 == zero2)
  inspect(zero1.to_string(), content="0")
  inspect(zero2.to_string(), content="0")
}

///|
test "decimal_financial_example" {
  // Example: Calculate tax and total for a purchase
  let price = match @decimal.Decimal::from_string("19.99") {
    Some(d) => d
    None => fail("Failed to parse")
  }
  let tax_rate = match @decimal.Decimal::from_string("0.08") {
    Some(d) => d
    None => fail("Failed to parse")
  } // 8% tax
  let tax = price * tax_rate
  let total = price + tax
  inspect(price.to_string(), content="19.99")
  inspect(tax.to_string(), content="1.5992")
  inspect(total.to_string(), content="21.5892")

  // Round to 2 decimal places for currency
  let total_rounded = match total.round(2) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(total_rounded.to_string(), content="21.59")
}

///|
test "decimal_precision_limits" {
  // Test max_scale
  let max_scale_str = "0." + "1".repeat(@decimal.max_scale)
  let max_scale_decimal = match @decimal.Decimal::from_string(max_scale_str) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(max_scale_decimal.scale(), content="28")

  // Test exceeding max_scale should fail
  let too_many_places = "0." + "1".repeat(@decimal.max_scale + 1)
  let result = @decimal.Decimal::from_string(too_many_places)
  inspect(result, content="None")
}

///|
test "decimal_json_serialization" {
  let d = match @decimal.Decimal::from_string("123.45") {
    Some(d) => d
    None => fail("Failed to parse")
  }

  // Test to_json
  let json = d.to_json()
  inspect(
    json,
    content=(
      #|String("123.45")
    ),
  )

  // Test from_json
  let json_str = Json::string("67.89")
  let parsed : @decimal.Decimal = @json.from_json(json_str)
  inspect(parsed, content="67.89")
}

///|
test "decimal_error_handling" {
  // Test invalid strings
  let invalid1 = @decimal.Decimal::from_string("abc")
  inspect(invalid1, content="None")
  let invalid2 = @decimal.Decimal::from_string("12.34.56")
  inspect(invalid2, content="None")
  let invalid3 = @decimal.Decimal::from_string("")
  inspect(invalid3, content="None")

  // Test division by zero
  // let a = match @decimal.Decimal::from_string("10.0") {
  //   Some(d) => d
  //   None => fail("Failed to parse")
  // }
  // let zero = @decimal.zero()
  // let div_result = a.div(zero)
  // inspect(div_result, content="None")

  // Test invalid scale in new
  let invalid_scale1 = @decimal.Decimal::new(123N, -1)
  inspect(invalid_scale1, content="None")
  let invalid_scale2 = @decimal.Decimal::new(123N, @decimal.max_scale + 1)
  inspect(invalid_scale2, content="None")
}

///|
test "decimal_normalization" {
  // Test that trailing zeros are removed
  let d1 = match @decimal.Decimal::new(12300N, 3) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d1.to_string(), content="12.3")
  inspect(d1.coefficient(), content="123")
  inspect(d1.scale(), content="1")
  let d2 = match @decimal.Decimal::new(123000N, 4) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d2.to_string(), content="12.3")
  inspect(d2.coefficient(), content="123")
  inspect(d2.scale(), content="1")

  // Test zero normalization
  let d3 = match @decimal.Decimal::new(0N, 5) {
    Some(d) => d
    None => fail("Failed to parse")
  }
  inspect(d3.to_string(), content="0")
  inspect(d3.coefficient(), content="0")
  inspect(d3.scale(), content="0")
}
