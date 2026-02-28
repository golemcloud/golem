///|
const CDE : Bytes = b"cde"

fn match_bytes(x : Bytes) -> Unit {
  match x {
    b"abc" => println("abc")
    CDE => println("cde")
    _ => println("other")
  }
}

///|
fn main {
  let x = b"abc"
  match_bytes(x)
  if x is b"xxx" {
    println("xxx")
  } else {
    println("other")
  }
}
