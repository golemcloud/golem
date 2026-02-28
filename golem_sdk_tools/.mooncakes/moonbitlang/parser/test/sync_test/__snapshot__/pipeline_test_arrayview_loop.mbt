///|
fn[T] each(data : Array[T], f : (T) -> Unit) -> Unit {
  loop data[:] {
    [x, .. rest] => {
      f(x)
      continue rest
    }
    [] => ()
  }
}

///|
fn main {
  let v = [1, 2, 3]
  each(v, fn(xx) { println(xx) })
}
