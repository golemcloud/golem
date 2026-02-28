fn main {
  try {
    f!(x)
  } catch {
    E1 => println("x")
    e => raise e
  }
}
