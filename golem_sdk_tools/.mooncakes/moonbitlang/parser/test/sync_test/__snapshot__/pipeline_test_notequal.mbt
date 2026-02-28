
struct T {
  x: Int
}

impl Eq for T with equal(self: T, other: T) -> Bool {
  not(self.x != other.x)
}

fn init {
  let t = { x: 1 }
  if t != t {
    println("Boom!")
  } else {
    println("Ok!")
  }
}
