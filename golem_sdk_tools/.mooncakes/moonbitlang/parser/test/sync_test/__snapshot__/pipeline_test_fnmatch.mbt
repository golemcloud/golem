enum MList[T]{
  MNil
  MCons(T, MList[T])
} derive(Show)

fn init {
  fn zip_add(x, y) {
    match (x, y) {
      (MList::MNil, MList::MNil) => MList::MNil
      (MCons(x,xs), MCons(y, ys)) => MCons(x+y, zip_add(xs, ys))
      (_, _) => abort("invalid arg")
    }
  }
  fn pr(x) {
    match x {
      MList::MNil => ()
      MCons(x, MNil) => { println(x) }
      MCons(x, xs) => { println(x); println(","); pr(xs) }      
    }
  }
  let xs = zip_add(MCons(1, MCons(2, MNil)), (MCons(3, MCons(4, MNil))))
  println(xs)
  println("[")
  pr(xs)
  println("]")
}

fn[A, B, C] zip_with(xs: MList[A], ys: MList[B], f: (A, B) -> C) -> MList[C] {
  match (xs, ys) {
    (MCons(x, rest_x), MCons(y, rest_y)) => MCons(f(x, y), zip_with(rest_x, rest_y, f))
    (MNil, MNil) => MNil
    (_, _) => abort("invalid arg")
  }
}

fn init {
  let zs = zip_with(MCons(1, MCons(2, MNil)), (MCons(3, MCons(4, MNil))), (x, y) => (x, y))
  println(zs)
  fn split (x) {
    match x {
      MList::MNil => (MList::MNil, MList::MNil)
      MCons((a, b), rest) => {
        let (r1, r2) = split(rest)
        (MCons(a, r1), MCons(b, r2))
      }
    }
  }
  let (xs, ys) = split(zs)
  println(xs)
  println(ys)
}
