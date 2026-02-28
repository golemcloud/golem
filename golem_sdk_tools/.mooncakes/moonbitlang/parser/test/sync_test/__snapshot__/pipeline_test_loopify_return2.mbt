///| return in nested function. But g is inlined, so this is loopified.
fn loop1() -> Int {
  fn f(x : Int) {
    fn g(x : Int) {
      if x < 0 {
        return x
      }
      if x > 0 {
        return f(x - 1)
      }
      42
    }

    g(x) + 1
  }

  f(10) + f(-1)
}

///| return in nested function. Should not loopify
fn loop2() -> Int {
  fn f(x : Int) {
    fn g(x : Int) {
      if x < 0 {
        return x
      }
      if x > 0 {
        return f(x - 1)
      }
      42
    }

    ignore(g) // prevent inline g
    g(x) + 1
  }

  f(10) + f(-1)
}

///| return in nested function. Should not loopify
fn loop3() -> Int {
  fn f(x : Int) {
    letrec g=fn(x : Int) {
      if x < 0 {
        return h(x)
      }
      if x > 0 {
        return f(x - 1)
      }
      42
    }

    and h=fn(x : Int) {
      if x < 0 {
        return x
      } else {
        g(x)
      }
    }

    g(x) + 1
  }

  f(10) + f(-1)
}

///|
fn main {
  println(loop1())
  println(loop2())
  println(loop3())
}
