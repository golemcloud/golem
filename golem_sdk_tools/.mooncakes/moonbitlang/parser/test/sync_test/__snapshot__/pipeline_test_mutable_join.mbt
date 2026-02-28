fn test1() -> Int {
  let mut f = fn(x) { x + 1 }
  f(2)
}

fn test2() -> Int {
  let mut f = fn(x) { x + 1 }
  f = fn(x) { x + 10 }
  f(2)
}

fn init {
  println(test1())
  println(test2()) // expect to  print 12
}

