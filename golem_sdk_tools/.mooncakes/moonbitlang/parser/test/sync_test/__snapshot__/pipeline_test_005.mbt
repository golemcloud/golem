fn add(a: Int) -> (Int, Int) -> Int {
  fn(b, c) { a + b + c }
}

fn init {
  let add1 = add(111)
  let add2 = add(1111)
  println(add1(3, 4))
  println(add2(3, 4))
}

