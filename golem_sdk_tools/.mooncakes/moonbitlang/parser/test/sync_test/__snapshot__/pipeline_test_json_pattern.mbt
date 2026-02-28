//!build: -w -21
fn f1(x : Json) -> Double {
  match x {
    { "x": 42, .. } => 42
    { "x": [ 42, "42" ], .. } => 84
    { "x": Number(x), "y": Number(y), .. } => x + y
    _ => 0
  }
}

fn init {
  f1({ "x": 42, "y": 4 }) |> println
  f1({ "x": 3, "y": 4 })|> println
  f1({ "y": 0 }) |> println
  f1({ "x": [ 42, "42" ] }) |> println
}
