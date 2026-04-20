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
fn add_prefix_zero(s : String, len : Int) -> String {
  let buf = StringBuilder::new(size_hint=len)
  for i = s.length(); i < len; i = i + 1 {
    buf.write_char('0')
  }
  buf.write_string(s)
  buf.to_string()
}

///|
test "add_prefix_zero" {
  inspect(add_prefix_zero("", 2), content="00")
  inspect(add_prefix_zero("1", 0), content="1")
  inspect(add_prefix_zero("1", 1), content="1")
  inspect(add_prefix_zero("1", 2), content="01")
  inspect(add_prefix_zero("1", 3), content="001")
}

///|
fn remove_suffix_zero(s : StringView) -> StringView {
  loop s {
    [_] as x => x // keep one zero if all are zeros
    [.. head, '0'] => continue head
    x => x
  }
}

///|
///
/// Formats a nanosecond value as a fractional seconds string for ISO 8601 output.
///
/// Pads the nanosecond value to 9 digits, trims trailing zeros, and prepends a dot.
/// For example:
///   format_nanos(0L)           => ".0"
///   format_nanos(1000100L)     => ".0010001"
///   format_nanos(100_000_000L) => ".1"
///   format_nanos(110_000_000L) => ".11"
///
/// This is used for formatting fractional seconds in time and duration string outputs.
fn format_nanos(nanos : Int64) -> String {
  let nanos_str = nanos.to_string().pad_start(9, '0')
  StringView::add(".", remove_suffix_zero(nanos_str)).to_string()
}

///|
test "remove_suffix_zero && format_nanos" {
  inspect(remove_suffix_zero(""), content="")
  inspect(remove_suffix_zero("1000100"), content="10001")
  inspect(remove_suffix_zero("000"), content="0")
  inspect(format_nanos(0L), content=".0")
  inspect(format_nanos(1000100L), content=".0010001")
  inspect(format_nanos(100_000_000L), content=".1")
  inspect(format_nanos(110_000_000L), content=".11")
}

///|
fn add_suffix_zero(s : String, len : Int) -> String {
  let buf = StringBuilder::new(size_hint=len)
  buf.write_string(s)
  for i = s.length(); i < len; i = i + 1 {
    buf.write_char('0')
  }
  buf.to_string()
}

///|
test "add_suffix_zero" {
  inspect(add_suffix_zero("1", 0), content="1")
  inspect(add_suffix_zero("1", 1), content="1")
  inspect(add_suffix_zero("1", 2), content="10")
  inspect(add_suffix_zero("1", 3), content="100")
}

///|
/// FIXME: use split method of String
fn split(s : String, delimiter : Char) -> Array[String] {
  let spl = []
  if s.length() == 0 {
    return spl
  }
  let buf = StringBuilder::new(size_hint=0)
  let delimiter_code = Char::to_int(delimiter).to_uint16()
  for i = 0; i < s.length(); i = i + 1 {
    let code_unit = s.code_unit_at(i)
    if code_unit == delimiter_code {
      spl.push(buf.to_string())
      buf.reset()
    } else {
      buf.write_char(UInt16::unsafe_to_char(code_unit))
    }
  }
  spl.push(buf.to_string())
  spl
}

///|
test "split" {
  inspect(
    split("2000-01-01", '-'),
    content=(
      #|["2000", "01", "01"]
    ),
  )
  inspect(
    split("-01-01", '-'),
    content=(
      #|["", "01", "01"]
    ),
  )
  inspect(
    split("12-31", '-'),
    content=(
      #|["12", "31"]
    ),
  )
  inspect(
    split("-01", '-'),
    content=(
      #|["", "01"]
    ),
  )
  inspect(
    split("-", '-'),
    content=(
      #|["", ""]
    ),
  )
  inspect(split("", '-'), content="[]")
  inspect(
    split("0", '-'),
    content=(
      #|["0"]
    ),
  )
}

///|
fn[T] int_overflow_err() -> T raise Error {
  raise Failure::Failure("Int overflow")
}

///|
fn[T] int64_overflow_err() -> T raise Error {
  raise Failure::Failure("Int64 overflow")
}

///|
/// FIXME: use checked_add method of Int
fn checked_add_int(x : Int, y : Int) -> Int raise Error {
  let r = x.to_int64() + y.to_int64()
  if r < @int.min_value.to_int64() || r > @int.max_value.to_int64() {
    int_overflow_err()
  } else {
    r.to_int()
  }
}

///|
/// FIXME: use checked_mul method of Int
fn checked_mul_int(x : Int, y : Int) -> Int raise Error {
  let r = x.to_int64() * y.to_int64()
  if r < @int.min_value.to_int64() || r > @int.max_value.to_int64() {
    int_overflow_err()
  } else {
    r.to_int()
  }
}

///|
/// FIXME: use checked_add method of Int64
fn checked_add_int64(x : Int64, y : Int64) -> Int64 raise Error {
  let r = x + y
  // Overflow iff both arguments have the opposite sign of the result
  if ((x ^ r) & (y ^ r)) < 0L {
    int64_overflow_err()
  } else {
    r
  }
}

///|
/// FIXME: use checked_mul method of Int64
fn checked_mul_int64(x : Int64, y : Int64) -> Int64 raise Error {
  let r = x * y
  let abs_x = x.abs()
  let abs_y = y.abs()
  if ((abs_x ^ abs_y).reinterpret_as_uint64() >> 31).reinterpret_as_int64() !=
    0L {
    // bits that greater than 2^31 might cause overflow
    if (y != 0L && r / y != x) || (x == @int64.min_value && y == -1L) {
      int64_overflow_err()
    }
  }
  r
}

///|
test "checked_add_int" {
  inspect(try? checked_add_int(1, 1), content="Ok(2)")
  inspect(try? checked_add_int(1, -1), content="Ok(0)")
  inspect(try? checked_add_int(-1, -1), content="Ok(-2)")
  inspect(
    try? checked_add_int(@int.max_value, 1),
    content=(
      #|Err(Failure("Int overflow"))
    ),
  )
  inspect(
    try? checked_add_int(@int.min_value, -1),
    content=(
      #|Err(Failure("Int overflow"))
    ),
  )
}

///|
test "checked_mul_int" {
  inspect(try? checked_mul_int(2, 3), content="Ok(6)")
  inspect(try? checked_mul_int(2, -3), content="Ok(-6)")
  inspect(try? checked_mul_int(-2, -3), content="Ok(6)")
  inspect(
    try? checked_mul_int(@int.max_value, 2),
    content=(
      #|Err(Failure("Int overflow"))
    ),
  )
  inspect(
    try? checked_mul_int(@int.min_value, 2),
    content=(
      #|Err(Failure("Int overflow"))
    ),
  )
}

///|
test "checked_add_int64" {
  inspect(try? checked_add_int64(1L, 1L), content="Ok(2)")
  inspect(try? checked_add_int64(1L, -1L), content="Ok(0)")
  inspect(try? checked_add_int64(-1L, -1L), content="Ok(-2)")
  inspect(
    try? checked_add_int64(1L, @int64.max_value),
    content=(
      #|Err(Failure("Int64 overflow"))
    ),
  )
  inspect(
    try? checked_add_int64(-1L, @int64.min_value),
    content=(
      #|Err(Failure("Int64 overflow"))
    ),
  )
}

///|
test "checked_mul_int64" {
  inspect(try? checked_mul_int64(2L, 3L), content="Ok(6)")
  inspect(try? checked_mul_int64(2L, -3L), content="Ok(-6)")
  inspect(try? checked_mul_int64(-2L, -3L), content="Ok(6)")
  inspect(
    try? checked_mul_int64(@int64.max_value, 2L),
    content=(
      #|Err(Failure("Int64 overflow"))
    ),
  )
  inspect(
    try? checked_mul_int64(@int64.min_value, 2L),
    content=(
      #|Err(Failure("Int64 overflow"))
    ),
  )
}

///|
fn floor(x : Double) -> Double {
  if x.is_pos_inf() || x.is_neg_inf() || x.is_nan() {
    return x
  }
  let n = x.to_int64()
  let d = n.to_double()
  if x >= 0.0 || x == d {
    d
  } else {
    d - 1.0
  }
}

///|
fn floor_div_int(x : Int, y : Int) -> Int raise Error {
  if y == 0 {
    raise Failure::Failure("division by zero")
  }
  let r = x.to_double() / y.to_double()
  floor(r).to_int()
}

///|
test "floor_div_int" {
  inspect(try? floor_div_int(1, 10), content="Ok(0)")
  inspect(try? floor_div_int(-1, 10), content="Ok(-1)")
  inspect(try? floor_div_int(9, 10), content="Ok(0)")
  inspect(try? floor_div_int(-9, 10), content="Ok(-1)")
  inspect(try? floor_div_int(0, 10), content="Ok(0)")
  inspect(
    try? floor_div_int(10, 0),
    content=(
      #|Err(Failure("division by zero"))
    ),
  )
}

///|
fn floor_div_int64(x : Int64, y : Int64) -> Int64 raise Error {
  if y == 0L {
    fail("division by zero")
  }
  let r = x.to_double() / y.to_double()
  floor(r).to_int64()
}

///|
test "floor_div_int64" {
  inspect(try? floor_div_int64(1L, 10L), content="Ok(0)")
  inspect(try? floor_div_int64(-1L, 10L), content="Ok(-1)")
  inspect(try? floor_div_int64(9L, 10L), content="Ok(0)")
  inspect(try? floor_div_int64(-9L, 10L), content="Ok(-1)")
  inspect(try? floor_div_int64(0L, 10L), content="Ok(0)")
  assert_true((try? floor_div_int64(10L, 0L)) is Err(_))
}

///|
fn floor_mod_int(x : Int, y : Int) -> Int raise Error {
  x - floor_div_int(x, y) * y
}

///|
test "floor_mod_int" {
  inspect(try? floor_mod_int(1, 10), content="Ok(1)")
  inspect(try? floor_mod_int(-1, 10), content="Ok(9)")
  inspect(try? floor_mod_int(11, 10), content="Ok(1)")
  inspect(try? floor_mod_int(-11, 10), content="Ok(9)")
  inspect(try? floor_mod_int(0, 10), content="Ok(0)")
  inspect(
    try? floor_mod_int(10, 0),
    content=(
      #|Err(Failure("division by zero"))
    ),
  )
}

///|
fn floor_mod_int64(x : Int64, y : Int64) -> Int64 raise Error {
  x - floor_div_int64(x, y) * y
}

///|
test "floor_mod_int64" {
  inspect(try? floor_mod_int64(1L, 10L), content="Ok(1)")
  inspect(try? floor_mod_int64(-1L, 10L), content="Ok(9)")
  inspect(try? floor_mod_int64(11L, 10L), content="Ok(1)")
  inspect(try? floor_mod_int64(-11L, 10L), content="Ok(9)")
  inspect(try? floor_mod_int64(0L, 10L), content="Ok(0)")
  assert_true((try? floor_mod_int64(10L, 0L)) is Err(_))
}
