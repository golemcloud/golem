///|
fn h(f : (Int) -> Unit raise?) -> Unit raise? {
  f(42) catch {
    e => raise e
  }
  try {
    f(43)
    fail("hhh")
  } catch {
    _ => println("hhh")
  }
}

///|
suberror E {
  E(Int)
} derive(Show)

///|
fn main {
  println(try? h(fn(x) { raise E(x) }))
  h(fn(x) { println(x) })
}
