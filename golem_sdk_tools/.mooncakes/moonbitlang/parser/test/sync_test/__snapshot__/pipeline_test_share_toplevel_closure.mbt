///|
fn init {
  let a = 1
  let f = {
    ignore(1)
    g
  }
  f(a)
  let f = {
    ignore(1)
    g
  }
  f(a)
}

///|
fn[T] g(x : T) -> Unit {
  ignore(x)
}
