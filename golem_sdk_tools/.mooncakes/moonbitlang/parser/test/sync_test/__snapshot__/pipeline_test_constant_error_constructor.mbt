///|
suberror E derive(Show)

///|
pub fn f(a : Int, b : Int, e : Error) -> Unit {
  match (a, b, e) {
    (1, 1, _) => ()
    (_, 0, E as a) => println(a)
    _ => ()
  }
}

///|
fn main {
  f(0, 0, E)
}
