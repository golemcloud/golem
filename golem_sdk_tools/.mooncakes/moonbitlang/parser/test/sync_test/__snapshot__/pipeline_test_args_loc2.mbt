//!build: -w -27
///|
struct T(Int) derive(Show)

///|
let t : T = T(42)

///|
#callsite(autofill(loc, args_loc, dup))
fn f(
  self : T,
  x~ : Int,
  y~ : Int = 42,
  loc~ : SourceLoc,
  args_loc~ : ArgsLoc,
  dup~ : ArgsLoc,
) -> Unit {
  println("f( ~loc=\{loc}, ~args_loc=\{args_loc}), ~dup=\{dup}")
}

///|
fn init {
  T::f(t, x=2)
  T::f(x=2, t)
  T::f(y=3, x=2, t)
  t.f(x=3)
  t |> _.f(x=1, y=0)
}

///|
#callsite(autofill(loc, args_loc, dup))
fn g(
  self : T,
  x~ : Int,
  y~ : Int = 42,
  loc~ : SourceLoc?,
  args_loc~ : ArgsLoc?,
  dup~ : ArgsLoc?,
) -> Unit {
  println("f( ~loc=\{loc}, ~args_loc=\{args_loc}), ~dup=\{dup}")
}

///|
fn init {
  println("================")
  T::g(t, x=2, loc=None)
  T::g(t, x=2, args_loc=None, dup=None)
}
