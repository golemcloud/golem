///|
fn f1(x : StringView) -> Unit {
  match x {
    "abc" => println("abc")
    ['d', 'e', 'f'] => println("def")
    _ => println("other")
  }
}

///|
fn f2(x : BytesView) -> Unit {
  match x {
    "abc" => println("abc")
    ['d', 'e', 'f'] => println("def")
    _ => println("other")
  }
}

///|
fn main {
  f1("abc"[:])
  f1("0def0".view(start_offset=1, end_offset=4))
  f1("")
  f2(b"abc"[:])
  f2(b"0def0"[1:4])
  f2(b"")
}
