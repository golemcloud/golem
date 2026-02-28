fn init {
  println(1 + 2 * 3 + 4 - 5)
  if 2 * 2 + 3 == 9 / 2 + 10 / 3 {
    println("true")
  } else {
    println("false")
  }
  match 1 {
    0 => println(0)
    1 => println(1)
    x => println(x)
  }
  match 2 {
    0 => println(0)
    1 => println(1)
    x => println(x)
  }
  match true {
    true => println("true")
    false => println("false")
  }
  match Option::Some(1) {
    None => println(0)
    Some(x) => println(x + 1)
  }
  println(1 < 2)
  println(2 > 1)
  println(1 <= 2)
  println(2 >= 1)
  println(2 == 2)
  println(1 != 2)

  println(1L < 2)
  println(2L > 1)
  println(1L <= 2)
  println(2L >= 1)
  println(2L == 2)
  println(1L != 2)

  println(1U < 2)
  println(2U > 1)
  println(1U <= 2)
  println(2U >= 1)
  println(2U == 2)
  println(1U != 2)

  println(1UL < 2)
  println(2UL > 1)
  println(1UL <= 2)
  println(2UL >= 1)
  println(2UL == 2)
  println(1UL != 2)

  println((1 << 2) == 4)
  println((1L << 2) == 4)
  println((1U << 2) == 4)
  println((1UL << 2) == 4)

  println((16 >> 2) == 4)
  println((16L >> 2) == 4)
  println((16U >> 2) == 4)
  println((16UL >> 2) == 4)

  println((1 & 3) == 1)
  println((1L & 3) == 1)
  println((1U & 3) == 1)
  println((1UL & 3) == 1)
  
  println((1 | 2) == 3)
  println((1L | 2) == 3)
  println((1U | 2) == 3)
  println((1UL | 2) == 3)

  println((1 ^ 3) == 2)
  println((1L ^ 3) == 2)
  println((1U ^ 3) == 2)
  println((1UL ^ 3) == 2)
}
