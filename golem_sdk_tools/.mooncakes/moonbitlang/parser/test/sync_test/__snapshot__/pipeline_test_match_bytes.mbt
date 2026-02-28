///| basic usage
fn f1(b : Bytes, i : Int) -> Unit {
  match (b, i) {
    ([b'h',  .. rest,  b'o'], 42) => println(rest)
    ("world", 24) => println("world")
    ("hello", 24) => println("hello")
    _ => println("other")
  }
}

///| the very rare case where we get for_sure_yes for string literal
/// based on information from array patterns
fn f2(b : Bytes, i : Int) -> Unit {
  match (b, i) {
    ([b'a'..=b'z', ..], 42) => println("a-z")
    ([b'b'..=b'z', ..], 24) => println("b-z")
    (b"a", 24) => println("a")
    _ => println("other")
  }
}

///| test all byte literal patterns
fn f3(b : Bytes, i : Int) -> Unit {
  match (b, i) {
    ([b'h', b'e',  .. rest,  b'l', b'o'], 42) => println(rest)
    ([b'a', b'b', b'c'], 24) => println("abc")
    _ => println("other")
  }
}

///|
fn f5(b : Bytes) -> Unit {
  match b {
    [] | [_, _, .._] => println(1)
    [_..<b'a'] => println(2)
    [b'b'..<_] => println(3)
    b"a" => println(4)
  }
}

///|
fn main {
  f1(b"hello", 42)
  f1(b"hello", 24)
  f1(b"hhh", 24)
  f1(b"world", 24)
  f1(b"world", 42)
  f2(b"hello", 42)
  f2(b"hello", 24)
  f2(b"a", 24)
  f2(b"A", 24)
  f3(b"hello", 42)
  f3(b"hello", 24)
  f3(b"abc", 24)
  f5(b"")
  f5(b"hello")
  f5(b"a")
  f5(b"b")
  f5(b" ")
}
