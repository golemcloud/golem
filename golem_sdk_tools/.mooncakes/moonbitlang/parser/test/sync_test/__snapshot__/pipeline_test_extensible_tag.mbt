suberror E1

fn f1() -> Unit raise Error {
  raise E1
}

fn main {
  try f1() catch {
    E1 => println("a")
    InspectError::InspectError(s) => println(s)
    _ => println("b")
  }
}
