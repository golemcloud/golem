fn[T:Compare] f1(x: T, y: T) -> Unit {
  if x < y {
    println(".")
  } else {
    println("x")
  }
}

fn[T:Compare] f2(x: T, y: T) -> Unit {
  if x > y {
    println(".")
  } else {
    println("x")
  }
}

fn[T:Compare] f3(x: T, y: T) -> Unit {
  if x <= y {
    println(".")
  } else {
    println("x")
  }
}

fn[T:Compare] f4(x: T, y: T) -> Unit {
  if x >= y {
    println(".")
  } else {
    println("x")
  }
}

fn init {
  f1(1, 2)
  f1(1L, 2L)
  f1(1.0, 2.0)
  f2(1, 2)
  f2(1L, 2L)
  f2(1.0, 2.0)
  f3(1, 2)
  f3(1L, 2L)
  f3(1.0, 2.0)
  f4(1, 2)
  f4(1L, 2L)
  f4(1.0, 2.0)
}

fn init {
  if (1 < 2) { println(".") } else { println("x") }
  if (1L < 2L) { println(".") } else { println("x") }
  if (1.0 < 2.0) { println(".") } else { println("x") }
  if (1 > 2) { println(".") } else { println("x") }
  if (1L > 2L) { println(".") } else { println("x") }
  if (1.0 > 2.0) { println(".") } else { println("x") }
  if (1 <= 2) { println(".") } else { println("x") }
  if (1L <= 2L) { println(".") } else { println("x") }
  if (1.0 <= 2.0) { println(".") } else { println("x") }
  if (1 >= 2) { println(".") } else { println("x") }
  if (1L >= 2L) { println(".") } else { println("x") }
  if (1.0 >= 2.0) { println(".") } else { println("x") }
}
