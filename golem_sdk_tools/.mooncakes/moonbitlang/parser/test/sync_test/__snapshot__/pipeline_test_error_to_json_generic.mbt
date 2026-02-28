///|
trait F {
  f(Self) -> String
}

///|
impl F for Error with f(self : Error) -> String = "%error.to_string"

///|
impl[A : F] F for A? with f(self) {
  match self {
    None => "None"
    Some(a) => F::f(a)
  }
}

///|
suberror T {
  T(Int)
}

///|
fn main {
  let a : Error? = Some(T(42))
  println(F::f(a))
}
