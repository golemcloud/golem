fn f(x: Int) -> Int {
  x + 1
}

fn init {
  println("test return at tail position")
  return
}

fn init {
  if f(10) > 10 {
    return
  }
  println("not ok")
}

fn init {
  if f(10) < 10 {
    return ()
  }
  println("ok")
}
