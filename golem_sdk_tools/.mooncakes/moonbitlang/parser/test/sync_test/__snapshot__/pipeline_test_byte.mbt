fn test_byte(b: Byte, i: Int) -> Unit {
  if b.to_int() == i {
    println(".")
  } else {
    println("x")
  }
}

fn init {
  test_byte(b'a', 97)
  test_byte(b'\xff', 255)
  test_byte(b'\o377', 255)
  test_byte(b'\n', 10)
  test_byte(b'\r', 13)
  test_byte(b'\t', 9)
  test_byte(b' ', 32)
  test_byte(b'\'', 39)
  test_byte(b'\"', 34)
  test_byte(b'"', 34)
  test_byte(1, 1)
  test_byte(0x23, 35)
}
