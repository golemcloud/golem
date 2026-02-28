suberror MyError {
  MyError(Int)
} derive(Show, Eq, Compare, Hash)

suberror MyError2 derive(Show, Eq, Compare, Hash)

fn f() -> Unit raise MyError {
  raise MyError(42)
}

fn f2() -> Unit raise MyError2 {
  raise MyError2
}

fn[T : Hash] stable_hash(value : T) -> Int {
  let hasher = Hasher::new(seed = 42)
  hasher.combine(value)
  hasher.finalize()
}

fn[X : Show + Eq + Compare + Hash] use_derived(e0 : X, e : X) -> Unit {
  println(e)
  println(e0 == e)
  println(e == e)
  println(e0.compare(e))
  println(e.compare(e0))
  println(e.compare(e))
  println(stable_hash(e))
  println(stable_hash(e0))
}

fn init {
  println("===")
  let e0 = MyError(0)
  try {
    f()
  } catch {
    e => use_derived(e0, e)
  }
}

fn init {
  println("===")
  let e0 = MyError2
  try {
    f2()
  } catch {
    e => use_derived(e0, e)
  }
}
