fn f(x: Int?, y: Int?) -> (Int, Int) {
  if x is None && y is None {
    println("both are None")
    (0, 0)
  } else {
    println("at least one is Some")
    (1, 1)
  }
}

fn f2(x: Int?, y: Int?) -> (Int, Int) {
  if x is None || y is None {
    println("at least one is None")
    (0, 0)
  } else {
    println("both are Some")
    (1, 1)
  }
}

///|
fn g(b : Bool) -> Unit {
  println(b)
}

fn h(x: Array[Int?]) -> Unit {
  guard x[0] is None && x[1] is None else { println("wrong") }
  println("first two are None")
}

///|
fn main {
  let (a, b) = f(None, None)
  g(a is 0 && b is 1)
  h([None, None, Some(1), Some(2), Some(3)])
  let (c, d) = (f2(Some(a), Some(b)))
  g(c is 1 || d is 1)
}
