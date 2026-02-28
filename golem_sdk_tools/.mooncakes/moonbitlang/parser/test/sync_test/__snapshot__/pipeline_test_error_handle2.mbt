enum List[X] {
  Nil
  Cons(X, List[X])
}

/// convert list to string
impl[T : Show] Show for List[T] with output(self, logger) {
  fn go(ls : List[T]){
    match ls {
      Nil => ()
      Cons(x,Nil) => x.output(logger)
      Cons(x,xs) => {
        x.output(logger)
        logger.write_string(", ")
        go(xs)
      }
    }
  }
  logger.write_string("[")
  go(self)
  logger.write_string("]")
}

fn[T] List::from_array(arr: Array[T]) -> List[T] {
  loop (arr.length() - 1, List::Nil) {
    (0, acc) => List::Cons(arr[0], acc)
    (i, acc) => continue (i - 1, List::Cons(arr[i], acc))
  }
}

fn[A, B, E : Error] map_with_error(xs: List[A], f: (A) -> B raise E) -> List[B] raise E {
  match xs {
    Nil => Nil
    Cons(x, rest) => {
      let b = f(x)
      let bs = map_with_error(rest, f)
      Cons(b, bs)
    }
  }
}

struct T {
  a: Error
  b: (Int) -> Int raise Error
}

fn init {
  let xs = List::from_array([1, 2, 3, 4])
  let ys = try! map_with_error(xs, fn (x) {
    if x < 5 { x + 1 } else { fail("too big") }
  })
  println(ys)
  let t = T::{
    a: Failure::Failure("1"),
    b: fn (x) { if x < 5 { x - 1 } else { fail("42") } },
  }
  let zs = try! map_with_error(xs, t.b)
  println(zs)
}
