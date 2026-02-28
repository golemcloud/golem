enum P[X] {
  X(x~ : X)
}

fn f(p1 : P[Int], p2 : P[String]) -> String {
  match (p1, p2) {
    (X(_) as p1, X(_) as p2) => p1.x.to_string() + ", " + p2.x
  }
}

fn init {
  f(X(x=42), X(x="abc")) |> println
}
