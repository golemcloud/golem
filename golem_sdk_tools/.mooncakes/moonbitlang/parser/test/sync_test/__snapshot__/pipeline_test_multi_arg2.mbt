enum T {
  A(Int, Int)
  B(Int)
}

pub fn f(t: T) -> Int {
  match t {
    A(1, x) | A(x, 2) => x
    A(3, 4) => 34
  }
}

pub fn g(t: T) -> Int {
  match t {
    B(1) | B(2) => 42
    B(y) => y
  }
}

fn init {
  let t: T = A(3, 4)
  println(f(t))
  let t: T = B(42)
  println(g(t))
}

enum S {
  A(Int, String)
}

pub fn h(s: S) -> Int {
  match s {
    A(x, y) => x + y.length()
  }
}

fn init {
  println(h(A(1, "xx")))
}
