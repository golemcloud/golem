
enum T {
  A(Int)
  B(Int)
}

fn f(t: T) -> Unit {
  let (A(x) | B(x)) = t
  println(x)
}

fn init {
  let (Some(x)) = Option::Some(1)
  let y = 2
  println(x + y)
  f(A(3))
  f(B(4))
}
