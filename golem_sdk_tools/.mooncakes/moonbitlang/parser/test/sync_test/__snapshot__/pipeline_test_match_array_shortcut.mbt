///|
fn f(x : BytesView) -> Unit {
  match x {
    [b'a', b'b', .._] => println(1)
    [b'b', y, z, .._] => println(y + z)
    _ => println(3)
  }
}

///|
fn g(x : Array[Int?]) -> Unit {
  match x {
    [Some(1), Some(2), .._] => println(1)
    [Some(2), None, Some(a), .._] => println(a)
    _ => println(3)
  }
}

///|
fn h(x : Array[Int?]) -> Unit {
  match x {
    [_, Some(2), .._] => println(1)
    [None, None, _, .._] => println(2)
    _ => println(3)
  }
}

///|
fn main {
  f(b"abc"[:])
  f(b"bcd"[:])
  f(b""[:])
  g([Some(1), Some(2), None])
  g([Some(2), None, Some(100)])
  g([Some(2), Some(3), Some(4)])
  h([Some(1), Some(2), None])
  h([Some(2), None, Some(100)])
  h([None, None, None])
}
