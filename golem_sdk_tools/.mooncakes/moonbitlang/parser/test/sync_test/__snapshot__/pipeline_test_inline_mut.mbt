///|
fn f(x : Int, y : Int) -> Int {
  x + y
}

///|
fn init {
  let mut x = 1
  f(
    x,
    {
      x = 2
      x
    },
  )
  |> println
}
