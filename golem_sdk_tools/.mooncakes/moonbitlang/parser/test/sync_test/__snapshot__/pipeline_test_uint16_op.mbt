///|
fn fadd(x : UInt16, y : UInt16) -> UInt16 {
  x + y
}

///|
fn fsub(x : UInt16, y : UInt16) -> UInt16 {
  x - y
}

///|
fn fmul(x : UInt16, y : UInt16) -> UInt16 {
  x * y
}

///|
fn fdiv(x : UInt16, y : UInt16) -> UInt16 {
  x / y
}

///|
fn feq(x : UInt16, y : UInt16) -> Bool {
  x == y
}

///|
fn fne(x : UInt16, y : UInt16) -> Bool {
  x != y
}

///|
fn fgt(x : UInt16, y : UInt16) -> Bool {
  x > y
}

///|
fn fge(x : UInt16, y : UInt16) -> Bool {
  x >= y
}

///|
fn flt(x : UInt16, y : UInt16) -> Bool {
  x < y
}

///|
fn fle(x : UInt16, y : UInt16) -> Bool {
  x <= y
}

///|
fn main {
  println("1 + 2: \{fadd(1, 2)}")
  println("5 - 3: \{fsub(5, 3)}")
  println("3 * 4: \{fmul(3, 4)}")
  println("10 / 2: \{fdiv(10, 2)}")

  // Test equality operators
  println("5 == 5: \{feq(5, 5)}")
  println("5 == 6: \{feq(5, 6)}")
  println("5 != 6: \{fne(5, 6)}")
  println("5 != 5: \{fne(5, 5)}")

  // Test comparison operators
  println("10 > 5: \{fgt(10, 5)}")
  println("5 > 10: \{fgt(5, 10)}")
  println("5 > 5: \{fgt(5, 5)}")
  println("10 >= 5: \{fge(10, 5)}")
  println("5 >= 5: \{fge(5, 5)}")
  println("5 >= 10: \{fge(5, 10)}")
  println("5 < 10: \{flt(5, 10)}")
  println("10 < 5: \{flt(10, 5)}")
  println("5 < 5: \{flt(5, 5)}")
  println("5 <= 10: \{fle(5, 10)}")
  println("5 <= 5: \{fle(5, 5)}")
  println("10 <= 5: \{fle(10, 5)}")

  // Test corner cases
  println("65535 + 1: \{fadd(65535, 1)}") // Max UInt16 + 1 (overflow)
  println("0 - 1: \{fsub(0, 1)}") // Min UInt16 - 1 (underflow)
  println("32767 * 2: \{fmul(32767, 2)}") // Overflow case
}
