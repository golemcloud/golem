///|
struct S(Int, S?) derive(Show, Eq, Hash, Default)

fn[T : Hash] stable_hash(value : T) -> Int {
  let hasher = Hasher::new(seed = 42)
  hasher.combine(value)
  hasher.finalize()
}

fn main {
  let s1 = S::default()
  let s2 = S::S(1, Some(s1))
  println(s2)
  println(s2 == s2)
  println(s1 == s2)
  println(stable_hash(s2))
  println(f(s1))
  println(f(s2))
}

fn f(s: S) -> S {
  match s {
    S(_) as a => {
      println(a.0)
      a.1.unwrap_or(a)
    }
  }
}
