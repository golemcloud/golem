enum MyList {
  Cons(Int, MyList)
  Cons2(Int, MyList)
  Nil
}

fn init {
  fn sum(xs : MyList, n) {
    match xs {
      Cons(x, xs) | Cons2(x, xs) => sum(xs, n + x)
      Nil => n
    }
  }

  println(sum(Cons2(2, Cons(1, Nil)), 0))
}
