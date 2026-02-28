///|
fn f(bs : BytesView) -> Unit {
  match bs {
    [] => println("empty")
    [97..=105 as a, .. bs] => {
      println(a)
      println(bs)
    }
    _ => println("unmatched")
  }
}

///|
fn g(bs : BytesView) -> Unit {
  match bs {
    [104, 101, 108, 108, 111] => println("hello")
    [119, 111, 114, 108, 100] => println("world")
    [_,  .. bs,  _] => f(bs)
    _ => println("other")
  }
}

///|
fn main {
  f(b"hello"[:])
  g(b"hello"[:])
  g(b"world"[:])
  g(b"helloworld"[:5])
  g(b"helloworld"[5:])
  g(b"helloworld"[1:2])
  g(b"helloworld"[5:8])
}
