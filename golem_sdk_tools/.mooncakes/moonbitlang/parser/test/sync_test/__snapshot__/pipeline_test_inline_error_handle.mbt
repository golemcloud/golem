suberror E {
  E(Int)
}

pub fn f(x: Int) -> Unit raise {
  fn f1(x: Int) -> _ raise {
    if x > 0 {
      raise E(x)
    } else if x < 0 {
      fail(x.to_string())
    }
    0
  }
  println(f1(x))

}

pub fn g(x: Int) -> Int raise {
  fn f1(x: Int) -> _ raise {
    if x > 0 {
      raise E(x)
    } else if x < 0 {
      fail(x.to_string())
    }
    0
  }
  f1(x)

}

fn main {
  let a = try? f(42)
  println(a)
  let b = try? g(0)
  println(b)
}
