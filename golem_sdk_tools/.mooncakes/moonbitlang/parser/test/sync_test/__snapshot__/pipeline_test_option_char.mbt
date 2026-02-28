enum T {
  A(Char?)
} derive (Show)

fn f(t: T) -> Unit {
  match t {
    A(Some(c)) => println(c)
    _ => println("none")
  }
}

fn g(t: Char?) -> Unit {
  match h(t) {
    Some(c) => println(c)
    _ => println("none")
  }
}

fn[T: Show] h(t: T) -> T {
  println(t)
  t
}

fn init {
  let t = T::A(Some('a'));
  f(t)
  println(t)
  g(Some('b'))
}
