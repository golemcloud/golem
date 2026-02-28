
///|
test "error recover 1" {
  x5 |> y => f(y) |> g
}

///|
test "error recover 2" {
  x5 |> (y) => { f(y) } |> g
}

///|
test "error recover 3" {
  x5 |> (y,z) => { f(y) } |> g
}

///|
test "error recover 4" {
  x5 |> (y,z) |> g
}

