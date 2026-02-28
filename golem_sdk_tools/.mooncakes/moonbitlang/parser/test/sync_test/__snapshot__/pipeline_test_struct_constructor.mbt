///|
struct S {
  x : Int
  y : Int

  #callsite(autofill(loc))
  fn new(x~ : Int, y? : Int, loc~ : SourceLoc) -> S raise
} derive(Show)

///|
#callsite(autofill(loc))
fn S::new(x~ : Int, y? : Int = x, loc~ : SourceLoc) -> S raise {
  if y == 42 {
    raise Failure::Failure("fourty-two")
  }
  println("S::new(x=\{x}, y=\{y}) at \{loc}")
  { x, y }
}

///|
fn init {
  println(try? S(x=1))
  println(try? S::S(x=2, y=3))
  println(try? (S(x=4, y=42) : S))
}
