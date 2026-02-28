struct MyInt(Int) derive(Show)
struct MyPair((Int, MyInt)) derive(Show)

enum E {
  A
}

trait I {
  f(Self, MyInt) -> MyPair
}

fn[T: I] toMyInt(x: T) -> MyPair {
  x.f(MyInt(0))
}

impl I for E with f(self: E, mi: MyInt) -> MyPair {
  match self {
    E::A => MyPair((0, mi))
  }
}

fn init {
  let x = toMyInt(E::A)
  println(x)
}
