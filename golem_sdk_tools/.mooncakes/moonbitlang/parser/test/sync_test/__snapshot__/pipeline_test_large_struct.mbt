struct S {
  x1: Int
  x2: Int
  x3: Int
  x4: Int
  x5: Int
  x6: Int
  x7: Int
  x8: Int
  x9: Int
} derive(Default, Show)

fn f1(x: S) -> S {
   { ..x, x1: 100 }
}

fn main {
  let s0 = S::default()
  println((f1(s0)))
}
