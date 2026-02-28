///|
fn fadd(x : Int16, y : Int16) -> Int16 {
  x + y
}

///|
fn fsub(x : Int16, y : Int16) -> Int16 {
  x - y
}

///|
fn fmul(x : Int16, y : Int16) -> Int16 {
  x * y
}

///|
fn fdiv(x : Int16, y : Int16) -> Int16 {
  x / y
}

///|
fn feq(x : Int16, y : Int16) -> Bool {
  x == y
}

///|
fn fne(x : Int16, y : Int16) -> Bool {
  x != y
}

///|
fn fgt(x : Int16, y : Int16) -> Bool {
  x > y
}

///|
fn fge(x : Int16, y : Int16) -> Bool {
  x >= y
}

///|
fn flt(x : Int16, y : Int16) -> Bool {
  x < y
}

///|
fn fle(x : Int16, y : Int16) -> Bool {
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
  println("32767 + 1: \{fadd(32767, 1)}") // Max Int16 + 1 (overflow)
  println("-32768 - 1: \{fsub(-32768, 1)}") // Min Int16 - 1 (underflow)
  println("16384 * 2: \{fmul(16384, 2)}") // Overflow case
}
