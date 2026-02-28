///|
const Hello = b"Hello"

///|
const World : Bytes = [ 87, 111, 114, 108, 100 ]

///|
fn f(b : Bytes) -> Unit {
  match b {
    Hello => println("Hello")
    World => println("World")
    _ => println("Unknown")
  }
}

///|
fn main {
  f(Hello)
  f(b"other")
}
