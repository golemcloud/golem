pub fn[X : Eq] all_eq(a : X, b : X, c : X) -> Bool {
  a == b && b == c
}

pub(all) enum X {
  A
  B
}

pub(all) struct T {
  x: X
}

pub(all) struct S {
  a: Int
}

pub impl Eq for X with equal(self: X, other: X) -> Bool {
  match (self, other) {
    (A, A) | (B, B) => true
    _ => false
  }
}

pub impl Eq for T with equal(self : T, other : T) -> Bool {
  self.x == other.x
}

pub impl Eq for S with equal(self : S, other : S) -> Bool {
  self.a == other.a
}

fn init {
  let x = X::A
  let t = { x: B }
  if all_eq(x, x, x) && all_eq(t, t, t) {
      println("good")
  }
  let x = 1
  let t = { a: 1 }
  if all_eq(x, x, x) && all_eq(t, t, t) {
      println("good")
  }

}
