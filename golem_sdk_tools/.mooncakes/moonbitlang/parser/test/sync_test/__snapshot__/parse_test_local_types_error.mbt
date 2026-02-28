fn main {
  enum T1 {
    A(Int)
  }
  println(42)
  enum T2 {
    A(Int)
  }
}

fn main {
  enum T1[X] {
    A(X)
  }
  println(42)
}

fn main {
  fn f() {
    enum T1 {
      A
    }
    println(42)
  }
  f()
}
