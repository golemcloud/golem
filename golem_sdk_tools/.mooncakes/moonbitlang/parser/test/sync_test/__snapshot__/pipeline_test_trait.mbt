
struct T { t: Int }
struct U { u: Int }
struct L[X] { x: X }

// should not deleted
impl Eq for T with equal(self: T, other: T) -> Bool {
  self.t == other.t
}

// should delete
fn T::equal(self: T, other: T) -> Bool {
  self.t == other.t
}

// should not deleted
impl Eq for U with equal(self: U, other: U) -> Bool {
  self.u == other.u
}

// should delete
fn U::equal(self: U, other: U) -> Bool {
  self.u == other.u
}

// should not deleted
impl[X: Eq] Eq for L[X] with equal(self, other) {
  self.x == other.x
}

// should delete
fn[X: Eq] L::equal(self: L[X], other: L[X]) -> Bool {
  self.x == other.x
}

fn[X: Eq] f(x: X) -> Unit {
  if x == x { println("O") } else { println("X") }
}

fn T::default() -> T {
  { t: 0 }
}

fn init {
  f({ t: 1 })
  f({ x: { u: 2 }})
  let _ = T::default()

}
