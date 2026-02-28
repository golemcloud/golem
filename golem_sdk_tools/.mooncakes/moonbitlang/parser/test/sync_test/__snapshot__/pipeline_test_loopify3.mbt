///|
fn main {
  fn loop1(x : Int) {
    letrec f = fn(x) {
      if x < 0 {
        return 42
      } else {
        g(x - 1)
      }
    }

    and g = fn(x) {
      if x < 0 {
        return 42
      } else {
        f(x - 1)
      }
    }

    if x > 0 {
      if x > 5 {
        loop1(x - 2)
        println("xxx")
      } else {
        loop1(x - 1)
      }
    } else {
      println(g(10))
    }
  }

  loop1(10)
}
