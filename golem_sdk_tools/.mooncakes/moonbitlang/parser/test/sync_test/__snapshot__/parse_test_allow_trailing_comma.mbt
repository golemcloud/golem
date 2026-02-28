enum Lst[T] {
  Nil
  Cons(
    T,
    Lst[T],
  )
}

fn[
  T,
  U,
] zip(
  xs : Lst[T],
  ys : Lst[U],
){
  match (
    xs,
    ys,
  ){
    (
      Cons(x,xs),
      Cons(y,ys),
    ) => {
      Cons(
        (
          x,
          y,
        ),
        zip(
          xs,
          ys,
        ),
      )
    }
    _ => Nil
  }
}

fn xtest_array() {
  let arr = [
    41,
    42,
    43,
  ]

  match arr {
    [
      x,
    ] => ()
    [
      x,
      y,
      z,
    ] => ()
    [
      ..,
      y,
      z,
    ] => ()
    [
      x,
      y,
      ..,
    ] => ()
    [
      ..,
    ] => ()
  }
}

fn xtest_func() {
  let f : (
    Int,
    Int,
  ) -> Int = 
    fn(
      x,
      y,
    ){
      x + y
    }

  f(
    41,
    42,
  )
}
