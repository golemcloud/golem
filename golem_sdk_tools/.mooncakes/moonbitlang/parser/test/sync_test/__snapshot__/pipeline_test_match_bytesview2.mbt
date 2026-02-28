///|
fn f(bs : BytesView) -> Unit {
  match bs {
    ['a',  .. rest,  'z'] => println(rest)
    ['b', 'c',  .. rest,  'x', 'y'] => println(rest)
    _ => println("other")
  }
}

///|
fn main {
  f(b"abcxyz"[:])
  f(b"abcxyz"[1:5])
  f(b"bcxyza"[:])
  f(b"bc123xy"[:])
}
