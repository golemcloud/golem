//!build: -w -27
struct T(Int) derive(Show)

let t : T = T(42)

fn f(
  self : T,
  x~ : Int,
  y~ : Int = 42,
  loc~ : SourceLoc = _,
  args_loc~ : ArgsLoc = _,
  dup~ : ArgsLoc = _
) -> Unit {
  println("f( ~loc=\{loc}, ~args_loc=\{args_loc}), ~dup=\{dup}")
}

fn init {
  T::f(t, x=2)
  T::f(x=2, t)
  T::f(y=3, x=2, t)
  t.f(x=3)
  t |> _.f(x=1, y=0)
}

fn g(
  j1 : Json,
  j2 : Json,
  j3 : Json,
  j4 : Json,
  map : Map[Int, Int],
  args_loc~ : ArgsLoc = _
) -> Unit {
  println("\{args_loc}")
}

fn init {
  g(true, "42", [ 1, 2 ], { "x": [] }, {})
}

fn interp(
  _ : String,
  args_loc~ : ArgsLoc = _
) -> Unit {
  println("interp(\{args_loc})")
}

fn init {
  interp(
    $| xxx \{42} xxx
  )
  interp("xxx \{42} xxx")
}

fn spread(_ : Array[Int], args_loc~ : ArgsLoc = _) -> Unit {
  println("spread(\{args_loc})")
}

fn init {
  let arr1 = [ 1, 2, 3 ]
  spread([ 0, ..arr1, 4 ])
}
