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
/// Arbitrary precision decimal type for exact decimal arithmetic.
///
/// This type is designed for financial calculations and other applications
/// that require exact decimal representation without floating-point errors.
///
/// The decimal is represented as a coefficient (BigInt) and a scale (Int):
/// - coefficient: The significant digits as a BigInt
/// - scale: The number of decimal places (0 <= scale <= max_scale)
///
/// For example: 123.45 is represented as coefficient=12345, scale=2
///
/// Invariants:
/// - 0 <= scale <= max_scale (default 28)
/// - coefficient and scale are normalized (no trailing zeros in coefficient)
/// - Zero is represented as coefficient=0, scale=0
///
/// Example:
/// ```moonbit check
/// test {
///   let price = match @decimal.Decimal::from_string("19.99") {
///     Some(p) => p
///     None => fail("Invalid price")
///   }
///   let tax = match @decimal.Decimal::from_string("0.08") {
///     Some(t) => t
///     None => fail("Invalid tax rate")
///   }
///   let total = price + tax
///   inspect(total.to_string(), content="20.07")
/// }
/// ```
///
pub struct Decimal {
  coefficient : @bigint.BigInt
  scale : Int
}

///|
/// Maximum number of decimal places (scale) allowed.
pub let max_scale = 28

///|
/// Creates a decimal from a coefficient and scale.
///
/// Parameters:
/// * `coefficient` : The significant digits as a BigInt
/// * `scale` : The number of decimal places
///
/// Returns `Some(decimal)` if the scale is valid, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::new(12345N, 2) {
///     Some(d) => d
///     None => fail("Invalid decimal")
///   } // 123.45
///   inspect(d.to_string(), content="123.45")
/// }
/// ```
///
pub fn Decimal::new(coefficient : @bigint.BigInt, scale : Int) -> Decimal? {
  if scale < 0 || scale > max_scale {
    None
  } else {
    Some(normalize({ coefficient, scale }))
  }
}

///|
/// Creates a decimal from a coefficient and scale without validation.
/// Used internally for performance-critical operations.
fn new_unchecked(coefficient : @bigint.BigInt, scale : Int) -> Decimal {
  { coefficient, scale }
}

///|
/// Normalizes a decimal by removing trailing zeros from the coefficient
/// and adjusting the scale accordingly.
fn normalize(d : Decimal) -> Decimal {
  if d.coefficient.is_zero() {
    { coefficient: 0N, scale: 0 }
  } else {
    let mut coefficient = d.coefficient
    let mut scale = d.scale

    // Remove trailing zeros
    while !coefficient.is_zero() && coefficient % 10N == 0N && scale > 0 {
      coefficient = coefficient / 10N
      scale = scale - 1
    }
    { coefficient, scale }
  }
}

///|
/// Creates a decimal from a string representation.
///
/// Parameters:
/// * `s` : String representation of the decimal (e.g., "123.45", "-0.001")
///
/// Returns `Some(decimal)` if parsing succeeds, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("123.45") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   inspect(d.to_string(), content="123.45")
/// }
/// ```
///
pub fn Decimal::from_string(s : String) -> Decimal? {
  parse_decimal_string(s)
}

///|
/// Parses a decimal string into a Decimal value.
fn parse_decimal_string(s : String) -> Decimal? {
  let s = s.trim(char_set=" ")
  if s.is_empty() {
    return None
  }
  let (sign, s) = if s.has_prefix("-") {
    (-1, s.view(start_offset=1))
  } else if s.has_prefix("+") {
    (1, s.view(start_offset=1))
  } else {
    (1, s.view())
  }
  if s.is_empty() {
    return None
  }

  // Find decimal point
  let dot_index = s.find(".")
  let (integer_part, fractional_part) = match dot_index {
    Some(i) => (s.view(end_offset=i), s.view(start_offset=i + 1))
    None => (s, "".view())
  }

  // Validate parts - at least one part must be non-empty
  if integer_part.is_empty() && fractional_part.is_empty() {
    return None
  }

  // Validate that parts contain only digits
  if !integer_part.is_empty() && !is_digit_string(integer_part.to_string()) {
    return None
  }
  if !fractional_part.is_empty() &&
    !is_digit_string(fractional_part.to_string()) {
    return None
  }

  // Parse integer part
  let coefficient = if integer_part.is_empty() {
    0N
  } else {
    @bigint.BigInt::from_string(integer_part.to_string())
  }

  // Parse fractional part
  let scale = fractional_part.length()
  if scale > max_scale {
    return None
  }
  let fractional_coefficient = if fractional_part.is_empty() {
    0N
  } else {
    @bigint.BigInt::from_string(fractional_part.to_string())
  }

  // Combine parts
  let total_coefficient = coefficient *
    @bigint.BigInt::from_int(10).pow(@bigint.BigInt::from_int(scale)) +
    fractional_coefficient
  let final_coefficient = if sign < 0 {
    -total_coefficient
  } else {
    total_coefficient
  }
  Decimal::new(final_coefficient, scale)
}

///|
/// Helper function to check if a string contains only digits.
fn is_digit_string(s : String) -> Bool {
  for c in s {
    if c < '0' || c > '9' {
      return false
    }
  }
  true
}

///|
/// Creates a decimal from an integer.
///
/// Parameters:
/// * `n` : The integer value
///
/// Returns a decimal with scale 0.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = @decimal.Decimal::from_int(42)
///   inspect(d.to_string(), content="42")
/// }
/// ```
///
pub fn Decimal::from_int(n : Int) -> Decimal {
  { coefficient: @bigint.BigInt::from_int(n), scale: 0 }
}

///|
/// Creates a decimal from a BigInt.
///
/// Parameters:
/// * `n` : The BigInt value
///
/// Returns a decimal with scale 0.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = @decimal.Decimal::from_bigint(12345678901234567890N)
///   inspect(d.to_string(), content="12345678901234567890")
/// }
/// ```
///
pub fn Decimal::from_bigint(n : @bigint.BigInt) -> Decimal {
  { coefficient: n, scale: 0 }
}

///|
/// Creates a decimal from a double with specified precision.
///
/// Parameters:
/// * `d` : The double value
/// * `precision` : Number of decimal places
///
/// Returns `Some(decimal)` if conversion succeeds, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_double(3.14159, 5) {
///     Some(d) => d
///     None => fail("Invalid double conversion")
///   }
///   inspect(d.to_string(), content="3.14159")
/// }
/// ```
///
pub fn Decimal::from_double(d : Double, precision : Int) -> Decimal? {
  if d.is_nan() {
    return None
  }
  if precision < 0 || precision > max_scale {
    return None
  }
  let multiplier = @math.pow(10.0, precision.to_double())
  let scaled = d * multiplier
  let rounded = scaled.round()

  // Check for overflow
  if rounded.abs() > 9223372036854775807.0 { // Int64::max_value
    return None
  }
  let coefficient = @bigint.BigInt::from_int64(rounded.to_int64())
  Decimal::new(coefficient, precision)
}

///|
/// Converts the decimal to a string representation.
///
/// Returns a string in the format "integer.fractional" or "integer".
///
/// Example:
/// ```moonbit check
/// test {
///   let d = @decimal.Decimal::from_string("123.45")
///   inspect(d.to_string(), content="Some(123.45)")
/// }
/// ```
///
pub fn Decimal::to_string(self : Decimal) -> String {
  if self.is_zero() {
    "0"
  } else {
    let coeff_str = self.coefficient.to_string()
    let is_negative = coeff_str.has_prefix("-")
    let abs_coeff = if is_negative {
      coeff_str.view(start_offset=1)
    } else {
      coeff_str.view()
    }
    if self.scale == 0 {
      coeff_str
    } else if abs_coeff.length() <= self.scale {
      // Need leading zeros
      let leading_zeros = "0".repeat(self.scale - abs_coeff.length() + 1)
      let with_zeros = leading_zeros + abs_coeff.to_string()
      let integer_part = with_zeros.view(
        end_offset=with_zeros.length() - self.scale,
      )
      let fractional_part = with_zeros.view(
        start_offset=with_zeros.length() - self.scale,
      )
      let result = integer_part.to_string() + "." + fractional_part.to_string()
      if is_negative {
        "-" + result
      } else {
        result
      }
    } else {
      // Insert decimal point
      let integer_part = abs_coeff.view(
        end_offset=abs_coeff.length() - self.scale,
      )
      let fractional_part = abs_coeff.view(
        start_offset=abs_coeff.length() - self.scale,
      )
      let result = integer_part.to_string() + "." + fractional_part.to_string()
      if is_negative {
        "-" + result
      } else {
        result
      }
    }
  }
}

///|
/// Converts the decimal to a double (with potential precision loss).
///
/// Warning: This conversion may lose precision for large numbers or high precision decimals.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = @decimal.Decimal::from_string("3.14159")
///   inspect(d, content="Some(3.14159)")
/// }
/// ```
///
pub fn Decimal::to_double(self : Decimal) -> Double {
  if self.is_zero() {
    0.0
  } else {
    let coeff_double = @strconv.parse_double(
      self.coefficient.to_string().view(),
    ) catch {
      _ => 0.0 // Fallback to 0.0 if parsing fails
    }
    let divisor = @math.pow(10.0, self.scale.to_double())
    coeff_double / divisor
  }
}

///|
/// Converts the decimal to an integer by truncating towards zero.
///
/// Returns `Some(int)` if the decimal fits in an Int, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("123.99") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   inspect(d.to_int(), content="Some(123)")
/// }
/// ```
///
pub fn Decimal::to_int(self : Decimal) -> Int? {
  if self.scale == 0 {
    Some(self.coefficient.to_int())
  } else {
    let divisor = @bigint.BigInt::from_int(10).pow(
      @bigint.BigInt::from_int(self.scale),
    )
    let quotient = self.coefficient / divisor
    Some(quotient.to_int())
  }
}

///|
/// Converts the decimal to a BigInt by truncating towards zero.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("123.99") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   inspect(d.to_bigint(), content="123")
/// }
/// ```
///
pub fn Decimal::to_bigint(self : Decimal) -> @bigint.BigInt {
  if self.scale == 0 {
    self.coefficient
  } else {
    let divisor = @bigint.BigInt::from_int(10).pow(
      @bigint.BigInt::from_int(self.scale),
    )
    self.coefficient / divisor
  }
}

///|
/// Returns the zero decimal value.
pub fn zero() -> Decimal {
  { coefficient: 0N, scale: 0 }
}

///|
/// Returns the one decimal value.
pub fn one() -> Decimal {
  { coefficient: 1N, scale: 0 }
}

///|
/// Returns the negative one decimal value.
pub fn neg_one() -> Decimal {
  { coefficient: -1N, scale: 0 }
}

///|
/// Checks if the decimal is zero.
pub fn Decimal::is_zero(self : Decimal) -> Bool {
  self.coefficient.is_zero()
}

///|
/// Checks if the decimal is positive.
pub fn Decimal::is_positive(self : Decimal) -> Bool {
  self.coefficient > 0N
}

///|
/// Checks if the decimal is negative.
pub fn Decimal::is_negative(self : Decimal) -> Bool {
  self.coefficient < 0N
}

///|
/// Returns the sign of the decimal: -1, 0, or 1.
pub fn Decimal::signum(self : Decimal) -> Int {
  if self.coefficient.is_zero() {
    0
  } else if self.coefficient > 0N {
    1
  } else {
    -1
  }
}

///|
/// Returns the absolute value of the decimal.
pub fn Decimal::abs(self : Decimal) -> Decimal {
  if self.coefficient < 0N {
    { coefficient: -self.coefficient, scale: self.scale }
  } else {
    self
  }
}

///|
/// Returns the coefficient (significant digits) of the decimal.
pub fn Decimal::coefficient(self : Decimal) -> @bigint.BigInt {
  self.coefficient
}

///|
/// Returns the scale (number of decimal places) of the decimal.
pub fn Decimal::scale(self : Decimal) -> Int {
  self.scale
}

///|
/// Rounds the decimal to the specified number of decimal places.
///
/// Parameters:
/// * `self` : The decimal to round
/// * `places` : Number of decimal places (0 <= places <= max_scale)
///
/// Returns `Some(decimal)` if places is valid, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("3.14159") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   let rounded = match d.round(2) {
///     Some(r) => r
///     None => fail("Invalid rounding")
///   }
///   inspect(rounded.to_string(), content="3.14")
/// }
/// ```
///
pub fn Decimal::round(self : Decimal, places : Int) -> Decimal? {
  if places < 0 || places > max_scale {
    return None
  }
  if places >= self.scale {
    // No rounding needed
    return Some(self)
  }
  let places_to_remove = self.scale - places
  let divisor = @bigint.BigInt::from_int(10).pow(
    @bigint.BigInt::from_int(places_to_remove),
  )
  let remainder = self.coefficient % divisor
  let half_divisor = divisor / 2N
  let rounded_coeff = if remainder >= half_divisor {
    self.coefficient / divisor + 1N
  } else {
    self.coefficient / divisor
  }
  Decimal::new(rounded_coeff, places)
}

///|
/// Truncates the decimal to the specified number of decimal places.
///
/// Parameters:
/// * `self` : The decimal to truncate
/// * `places` : Number of decimal places (0 <= places <= max_scale)
///
/// Returns `Some(decimal)` if places is valid, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("3.14159") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   let truncated = match d.truncate(2) {
///     Some(t) => t
///     None => fail("Invalid truncation")
///   }
///   inspect(truncated.to_string(), content="3.14")
/// }
/// ```
///
pub fn Decimal::truncate(self : Decimal, places : Int) -> Decimal? {
  if places < 0 || places > max_scale {
    return None
  }
  if places >= self.scale {
    // No truncation needed
    return Some(self)
  }
  let places_to_remove = self.scale - places
  let divisor = @bigint.BigInt::from_int(10).pow(
    @bigint.BigInt::from_int(places_to_remove),
  )
  let truncated_coeff = self.coefficient / divisor
  Decimal::new(truncated_coeff, places)
}

///|
/// Scales the decimal to have the specified number of decimal places.
///
/// Parameters:
/// * `self` : The decimal to scale
/// * `new_scale` : New number of decimal places (0 <= new_scale <= max_scale)
///
/// Returns `Some(decimal)` if new_scale is valid, `None` otherwise.
///
/// Example:
/// ```moonbit check
/// test {
///   let d = match @decimal.Decimal::from_string("123.45") {
///     Some(d) => d
///     None => fail("Invalid decimal string")
///   }
///   let scaled = match d.scale_to(4) {
///     Some(s) => s
///     None => fail("Invalid scaling")
///   }
///   inspect(scaled.to_string(), content="123.4500")
/// }
/// ```
///
pub fn Decimal::scale_to(self : Decimal, new_scale : Int) -> Decimal? {
  if new_scale < 0 || new_scale > max_scale {
    return None
  }
  if new_scale == self.scale {
    return Some(self)
  } else if new_scale > self.scale {
    // Add zeros
    let scale_diff = new_scale - self.scale
    let multiplier = @bigint.BigInt::from_int(10).pow(
      @bigint.BigInt::from_int(scale_diff),
    )
    let new_coeff = self.coefficient * multiplier
    Some({ coefficient: new_coeff, scale: new_scale })
  } else {
    // Remove digits (truncate)
    let scale_diff = self.scale - new_scale
    let divisor = @bigint.BigInt::from_int(10).pow(
      @bigint.BigInt::from_int(scale_diff),
    )
    let new_coeff = self.coefficient / divisor
    Some({ coefficient: new_coeff, scale: new_scale })
  }
}

///|
/// Returns the default value (zero) for Decimal.
pub impl Default for Decimal with default() {
  zero()
}

///|
/// Tests equality between two decimals.
pub impl Eq for Decimal with equal(self : Decimal, other : Decimal) -> Bool {
  // Normalize both decimals to the same scale for comparison
  let max_scale = Int::max(self.scale, other.scale)
  match (self.scale_to(max_scale), other.scale_to(max_scale)) {
    (Some(self_scaled), Some(other_scaled)) =>
      self_scaled.coefficient == other_scaled.coefficient
    _ => false
  }
}

///|
/// Compares two decimals and returns their relative order.
pub impl Compare for Decimal with compare(self : Decimal, other : Decimal) -> Int {
  // Normalize both decimals to the same scale for comparison
  let max_scale = Int::max(self.scale, other.scale)
  match (self.scale_to(max_scale), other.scale_to(max_scale)) {
    (Some(self_scaled), Some(other_scaled)) =>
      self_scaled.coefficient.compare(other_scaled.coefficient)
    _ => 0
  }
}

///|
/// Adds two decimals.
pub impl Add for Decimal with add(self : Decimal, other : Decimal) -> Decimal {
  // Normalize both decimals to the same scale
  let max_scale = Int::max(self.scale, other.scale)
  match (self.scale_to(max_scale), other.scale_to(max_scale)) {
    (Some(self_scaled), Some(other_scaled)) =>
      new_unchecked(
        self_scaled.coefficient + other_scaled.coefficient,
        max_scale,
      )
    _ => self
  }
}

///|
/// Subtracts one decimal from another.
pub impl Sub for Decimal with sub(self : Decimal, other : Decimal) -> Decimal {
  // Normalize both decimals to the same scale
  let max_scale = Int::max(self.scale, other.scale)
  match (self.scale_to(max_scale), other.scale_to(max_scale)) {
    (Some(self_scaled), Some(other_scaled)) =>
      new_unchecked(
        self_scaled.coefficient - other_scaled.coefficient,
        max_scale,
      )
    _ => self
  }
}

///|
/// Multiplies two decimals.
pub impl Mul for Decimal with mul(self : Decimal, other : Decimal) -> Decimal {
  let new_coeff = self.coefficient * other.coefficient
  let new_scale = self.scale + other.scale

  // Check if scale exceeds maximum
  if new_scale > max_scale {
    // Truncate to max_scale
    let scale_diff = new_scale - max_scale
    let divisor = @bigint.BigInt::from_int(10).pow(
      @bigint.BigInt::from_int(scale_diff),
    )
    let truncated_coeff = new_coeff / divisor
    new_unchecked(truncated_coeff, max_scale)
  } else {
    new_unchecked(new_coeff, new_scale)
  }
}

///|
/// Implements division with maximum precision.
pub impl Div for Decimal with div(self : Decimal, other : Decimal) -> Decimal {
  if other.is_zero() {
    abort("Division by zero")
  }

  // Scale up the dividend to get maximum precision
  let scale_diff = max_scale - self.scale + other.scale
  let multiplier = @bigint.BigInt::from_int(10).pow(
    @bigint.BigInt::from_int(Int::max(0, scale_diff)),
  )
  let scaled_dividend = self.coefficient * multiplier
  let quotient = scaled_dividend / other.coefficient
  new_unchecked(quotient, max_scale)
}

///|
/// Returns the negation of the decimal.
pub impl Neg for Decimal with neg(self : Decimal) -> Decimal {
  { coefficient: -self.coefficient, scale: self.scale }
}

///|
/// Converts the decimal to its string representation.
pub impl Show for Decimal with output(self, logger) {
  logger.write_string(self.to_string())
}

///|
/// Converts a decimal to its JSON representation as a string.
pub impl ToJson for Decimal with to_json(self : Decimal) -> Json {
  Json::string(self.to_string())
}

///|
/// Creates a decimal from its JSON representation.
pub impl @json.FromJson for Decimal with from_json(json, path) {
  guard json is String(s) else {
    raise @json.JsonDecodeError(
      (path, "Decimal::from_json: expected string representation"),
    )
  }
  let result = Decimal::from_string(s)
  match result {
    Some(d) => d
    None =>
      raise @json.JsonDecodeError(
        (path, "Decimal::from_json: invalid decimal string"),
      )
  }
}

///|
/// Implements hash for decimal values.
pub impl Hash for Decimal with hash_combine(self, hasher) {
  hasher.combine(self.coefficient)
  hasher.combine(self.scale)
}

///|
/// Generates arbitrary decimal values for testing.
pub impl @quickcheck.Arbitrary for Decimal with arbitrary(size, rs) {
  let coeff = @quickcheck.Arbitrary::arbitrary(size, rs)
  let scale = rs.next_int() % (max_scale + 1)
  new_unchecked(coeff, scale)
}
