///|
suberror E {
  E(Int)
} derive(Show)

///|
fn e(x : Int) -> Int raise E {
  raise E(x)
}

///| test loopify
fn l(x : Int) -> Int raise {
  fn f(x : Int) -> Int raise {
    if x < 0 {
      raise E(x)
    } else if x > 1 {
      e(x) + 42
    } else {
      f(x + 1)
    }
  }

  f(x)
}

///| test contify
fn c(x : Int) -> Int raise {
  fn f(x : Int) -> Int raise {
    if x < 0 {
      raise E(x)
    } else if x > 1 {
      e(x) + 42
    } else {
      x
    }
  }

  if x is 100 {
    f(x)
  } else {
    f(x)
  }
}

///|
fn main {
  println(try? l(42))
  println(try? l(0))
  println(try? c(42))
  println(try? c(0))
}
