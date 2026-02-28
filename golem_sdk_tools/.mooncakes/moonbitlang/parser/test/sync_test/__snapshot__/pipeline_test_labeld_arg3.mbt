enum A {
  AA
  BB
}

fn[T] hello(x~ : T, y~ : T) -> (T, T) {
  (x, y)
}

fn init {
  hello(y=A::AA, x=BB) |> ignore
  // hello(y=AA, x=A::BB) |> ignore
}
