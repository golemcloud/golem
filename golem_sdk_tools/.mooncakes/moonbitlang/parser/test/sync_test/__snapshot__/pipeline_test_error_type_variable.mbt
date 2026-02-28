///|
fn[E : Error] f(x : E) -> Unit raise E {
  raise x
}

///|
fn[A, B] id(x : A) -> B = "%identity"

///|
suberror E derive(Show)

///|
fn main {
  f(id(E)) catch {
    e => println(e.to_string())
  }
}
