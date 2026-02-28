pub(all) struct T {
  x : Int
}

pub impl Add for T with add(self : T, other : T) -> T {
  { x: self.x + other.x }
}

let a : T = { x: 0 }

let b : T = a + a

fn init {
  println("hello")
}

