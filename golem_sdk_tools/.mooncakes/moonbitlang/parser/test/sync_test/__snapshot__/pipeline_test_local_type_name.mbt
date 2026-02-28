///|
pub(all) struct T(Int)

///|
pub(open) trait I {
  f(Self) -> Int
}

///|
pub fn T::g(self : T) -> Int {
  enum E {
    A(Int)
  } derive(Show)
  println(A(self.0))
  self.0
}

///|
pub impl I for T with f(self) {
  struct N(Int) derive(Show)
  println(N(self.0))
  self.0
}

///|
fn main {
  ignore(T::g(T(42)))
  ignore(I::f(T(42)))
}
