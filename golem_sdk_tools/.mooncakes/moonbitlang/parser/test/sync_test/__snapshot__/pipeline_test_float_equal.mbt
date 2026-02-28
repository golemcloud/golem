///|
fn main {
  let pos_zero : Double = 0.0
  let neg_zero : Double = -0.0
  println(pos_zero.reinterpret_as_int64())
  println(neg_zero.reinterpret_as_int64())

  // `if neg { -0.0 } else { 0.0 }` returns -0.0 even when neg=false.
  let neg = false
  let result = if neg { -0.0 } else { 0.0 }
  let bits = result.reinterpret_as_int64()
  // BUG: Should be 0, but MoonBit returns -9223372036854775808 (negative zero)
  println(bits)
}
