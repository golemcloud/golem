///|
struct Chain {
  value : Int
  mut next : Chain?
}

///|
fn Chain::next_unwrap(self : Chain) -> Chain {
  guard self.next is Some(next)
  next
}

///|
fn main {
  let cycle = { value: 42, next: None }
  cycle.next = Some(cycle)
  println(cycle.next_unwrap().value)
}
