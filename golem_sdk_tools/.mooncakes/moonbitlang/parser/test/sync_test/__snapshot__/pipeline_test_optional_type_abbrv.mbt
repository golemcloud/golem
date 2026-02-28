fn f(x : Int?) -> Array[Int]? {
  match x {
    None => None
    Some(x) => Some([x])
  }
}

fn init {
  f(None) |> println()
  f(Some(1)) |> println()
}

fn init {
  let mut x : (Double,Int)? = None
  x = Some((1.2, 5))
  println(x)
}
