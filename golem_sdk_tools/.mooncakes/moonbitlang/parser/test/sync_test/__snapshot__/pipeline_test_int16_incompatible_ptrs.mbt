///|
fn f(x : Int16) -> Int {
  x.to_int()
}

///|
fn g(x : Int) -> Int {
  x
}

///|
fn main {
  ignore(f)
  ignore(g)
}
