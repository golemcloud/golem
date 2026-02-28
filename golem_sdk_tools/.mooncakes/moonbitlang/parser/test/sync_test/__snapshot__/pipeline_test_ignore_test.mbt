fn init {
  let f = ignore
  3 |> f
}

fn init {
  let f = Int::add
  let a = x => f(x, 1)
  a(2) |> println
}
