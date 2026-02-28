///|
fn test_equal(a : Bytes, b : Bytes, res : Bool) -> Unit {
  if res == (a == b) {
    println("ok")
  } else {
    println("not ok")
  }
}

///|
fn main {
  // Basic cases
  test_equal(b"hello", b"hello", true)
  test_equal(b"hello", b"world", false)

  // Empty bytes
  test_equal(b"", b"", true)
  test_equal(b"", b"x", false)

  // Single byte
  test_equal(b"a", b"a", true)
  test_equal(b"a", b"b", false)

  // Test lengths near 8 bytes (important due to 64-bit word comparison)
  test_equal(b"1234567", b"1234567", true)
  test_equal(b"1234567", b"1234568", false)
  test_equal(b"12345678", b"12345678", true)
  test_equal(b"12345678", b"12345679", false)
  test_equal(b"123456789", b"123456789", true)
  test_equal(b"123456789", b"123456780", false)

  // Test with null bytes
  test_equal(b"\b00\b00\b00", b"\b00\b00\b00", true)
  test_equal(b"\b00abc\b00", b"\b00abc\b00", true)
  test_equal(b"\b00abc\b00", b"\b00abd\b00", false)

  // Test with all possible byte values
  test_equal(b"\bFF\b00\bFF", b"\bFF\b00\bFF", true)
  test_equal(b"\bFF\b00\bFF", b"\bFF\b01\bFF", false)

  // Long strings to test multiple 64-bit word comparisons
  test_equal(
    b"This is a long string to test multiple word comparisons!", b"This is a long string to test multiple word comparisons!",
    true,
  )
  test_equal(
    b"This is a long string to test multiple word comparisons!", b"This is a long string to test multiple word comparison?",
    false,
  )
}
