struct Arrow[T, B]((T) -> B)

fn call(arr : Arrow[Int, Int]) -> Unit {
  (arr.0)(1) |> println
}

fn increment(x : Int) -> Int {
  x + 1
}

fn main {
  call(Arrow(increment)) // Ok
  call(increment |> Arrow)
  let _ = increment |> Arrow
}
