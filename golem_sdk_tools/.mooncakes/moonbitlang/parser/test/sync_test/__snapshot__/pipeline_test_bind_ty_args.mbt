///|
priv struct A {
  b : (Int) -> Unit
  a : Int
}

///|
fn f() -> A {
  A::{ a: 0, b: ignore }
}

///|
fn main {
  let _ = f()

}
