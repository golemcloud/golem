///|
fn f(x : Int64) -> Unit {
  match x {
    100 => println("100")
    _..<10 => println("less than 10")
    11..=20 => println("between 11 and 20")
    21..<_ => println("greater than 20")
    _ => println("unknown")
  }
}

///|
fn g(x : UInt64) -> Unit {
  match x {
    100 => println("100")
    _..<10 => println("less than 10")
    11..=20 => println("between 11 and 20")
    21..<_ => println("greater than 20")
    _ => println("unknown")
  }
}

///|
fn h1(x : Int64) -> Unit {
  match x {
    0xFFFFFFFFFFFFFFFF => println("-1")
    0..=0 => println("zero")
    1..=10 => println("small positive")
    0xFFFFFFFFFFFFFFF0..=0xFFFFFFFFFFFFFFFE => println("-16 to -2")
    0x8000000000000000..=0x8000000000000010 =>
      println("-9223372036854775808 to -9223372036854775792")
    0xFFFFFFFFFFFFFF00..<0xFFFFFFFFFFFFFFFF => println("-256 to -1")
    _ => println("unknown")
  }
}

///|
fn h2(x : UInt64) -> Unit {
  match x {
    0xFFFFFFFFFFFFFFFF => println("max")
    0..=0 => println("zero")
    1..=10 => println("small positive")
    0xFFFFFFFFFFFFFFF0..=0xFFFFFFFFFFFFFFFE => println("big1")
    0x8000000000000000..=0x8000000000000010 => println("middle range")
    0xFFFFFFFFFFFFFF00..<0xFFFFFFFFFFFFFFFF => println("big2")
    _ => println("unknown")
  }
}

///|
fn main {
  f(100)
  f(9)
  f(11)
  f(20)
  f(21)
  f(1000)
  g(100)
  g(9)
  g(11)
  g(20)
  g(21)
  g(1000)
  h1(-1)
  h1(0)
  h1(10)
  h1(-8)
  h1(0x8000000000000003)
  h1(-250)
  h2(0xFFFFFFFFFFFFFFFF)
  h2(0)
  h2(10)
  h2(0xFFFFFFFFFFFFFFF0)
  h2(0xFFFFFFFFFFFFFFFE)
  h2(0x8000000000000000)
  h2(0x8000000000000010)
  h2(0xFFFFFFFFFFFFFF00)
}
