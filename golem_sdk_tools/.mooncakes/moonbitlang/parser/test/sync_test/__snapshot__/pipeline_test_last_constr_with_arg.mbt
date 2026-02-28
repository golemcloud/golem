
enum MyPair[T, U] {
  MyPair(T, U)
}

fn[T, U] MyPair::fst(self: MyPair[T, U]) -> T {
  let (MyPair(t, _)) = self
  t
}

fn[T, U] MyPair::snd(self: MyPair[T, U]) -> U {
  let (MyPair(_, u)) = self
  u
}

fn[X] get_opt(opt: X?, default: X) -> X {
  match opt {
    None => default
    Some(x) => x
  }
}

fn init {
  let p = MyPair::MyPair(1, MyPair::MyPair("2", 3))
  println(p.fst())
  println(p.snd().fst())
  println(p.snd().snd())
  println(get_opt(None, 0))
  println(get_opt(Some(5), 0))
}
