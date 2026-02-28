fn test1(x: Int) -> Unit {
  if x == 0 {
    println("x is 0")
  } else if x == 1 {
    println("x is 1")
  } else if x == 2 {
    println("x is 2")
  } else {
    println("x is not 0, 1 or 2")
  }
}

fn init {
  test1(0)
  test1(1)
  test1(2)
  test1(3)
}

