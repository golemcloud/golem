enum Lst {
  Empty
  Cons (Int, Lst)
}

fn init {
  let h : Lst = Cons(1, Empty)
  let Cons(x, Empty) = h
  println(x)
  match h {
    Empty => ()
    Cons(x, _) => println(x)
  }
  ()
}

