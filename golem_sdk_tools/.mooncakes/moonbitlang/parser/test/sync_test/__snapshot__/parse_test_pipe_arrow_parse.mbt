fn parsing_pipe_arrow_function() -> Unit {
  x1 |> f
  x2 |> y => { f(1, y) }
  x3 |> y => { f(1, y) } |> g |> println
  x4 |> y => { f(1, y) } |> z => { g(2, z) } |> println
}
