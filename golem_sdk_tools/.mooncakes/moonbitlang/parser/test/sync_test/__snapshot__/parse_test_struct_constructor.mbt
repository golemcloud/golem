///|
struct S {
  // empty
  fn new() -> S
}

///|
struct S {
  // trailing semi
  fn new() -> S;
}

///|
struct S {
  x : Int
  y : Int
  fn new(Int) -> S
}

///|
struct S {
  x : Int
  y : Int
  // trailing semi
  fn new(x : Int) -> S;
}

///|
struct S[X, Y] {
  x : X
  y : Y
  async fn[X : Eq, Y] new() -> S[X, Y] raise
}

///|
struct S {
  x : Int
  y : Int

  /// docstring for constructor
  #callsite(autofill(loc))
  fn new(x~ : Int, y? : Int, loc~ : SourceLoc) -> S
}

///|
struct S {
  // invalid name
  fn constr() -> S
}
