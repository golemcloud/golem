///|
const B : Bytes = b"xyz"

///|
const S : String = "123"

///|
fn f(x : BytesView) -> Unit {
  match x {
    [.. b"abc", .. rest] => println(rest)
    [.. B, .. rest] => println(rest)
    _ => println("other")
  }
}

///|
fn g(x : StringView) -> Unit {
  match x {
    [.. "abc", .. rest] => println(rest)
    [.. S, .. rest] => println(rest)
    _ => println("other")
  }
}

///|
fn main {
  f(b"abcxyz"[:])
  f(b"abcxyz"[3:])
  f(b"abcxyz"[6:])
  g("abcxyz"[:])
  try! g("abcxyz"[3:])
  try! g("abcxyz"[6:])
}
