///|
fn f(b : Byte) -> Unit {
  match b {
    b'a'..=b'z' => println("a-z")
    b'A' => println("A")
    _ => println("unmatched")
  }
}

///|
fn main {
  f(b'a')
  f(b'A')
  f(b'z')
  f(b'Z')
}
