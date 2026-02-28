fn[T: Show] f(z: T) -> Unit {
  fn f1(a: Int) {
    fn f2(b: Int) {
      if b == 1 { 1 } else { b * f1(b-1) }
    }
    if a == 1 { 1 } else { ignore(f2(1)); a * f2(a-1) }
  }
  println(z)
  println(f1(5) + f1(6))
}

fn init {
  f("5! + 6! = ")
}
