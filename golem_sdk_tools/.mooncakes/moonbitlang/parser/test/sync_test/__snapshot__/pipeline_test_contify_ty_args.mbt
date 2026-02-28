///|
struct A[T](T)

///|
fn[T : Show] A::f(self : A[T]) -> Unit {
  if self.0.to_string() == "" {

  } else {
    A("").f()
  }
}

///|
fn main {
  A(1).f()
}
