//!build: -w -27

fn f(xs : FixedArray[Int], start~ : Int, end~ : Int = xs.length()) -> Unit {
  let length = xs.length()
  println("length=\{length}, start=\{start}, end=\{end}")
}

fn init {
  println("===")
  f([1, 2, 3], start=0)
  f([1, 2, 3], start=1)
  f([1, 2, 3], start=1, end=2)
}

fn g(x : Int, y~ : Int = x * x, z~ : Int = y - x) -> Unit {
  println(z)
}

fn init {
  println("===")
  g(2)
  g(2, y=5)
  g(y=5, 2)
  g(z=1, y=5, 2)
  g(2, z=1, y=5)
}
