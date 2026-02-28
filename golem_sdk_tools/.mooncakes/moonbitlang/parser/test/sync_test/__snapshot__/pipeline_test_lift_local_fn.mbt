fn[A: Show, B: Show, C: Show] f(a: A, b: B, c: C) -> Unit {
  fn f1(a: A) {
    println(a)
  }
  fn f2(b: B, c: C) {
    println(b)
    fn f3() {
      println(42)
    }
    letrec f4 = fn(b: B, c: C, n: Int) {
      if n < 0 { return }
      println(b)
      f5(b, c, n-1)
    }
    and f5 = fn(b: B, c: C, n: Int) {
      if n < 0 { return }
      println(c)
      f4(b, c, n-1)
    }
    f3()
    f4(b, c, 3)
    f5(b, c, 4)
  }
  f1(a)
  f2(b, c)
  println("end")
}

fn init {
  f(42, "x", 3.14)
  f(3.15, 43, "xx")
}

fn init {
  letrec f1 = fn(x: Char) { f2(x) }
  and f2 = fn(x: Char) { f3(x) }
  and f3 = fn(x: Char) { println(x) }
  let _ = f1('c')
}
