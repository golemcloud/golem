
enum T {
  B(Int?, Int?)
}

fn f(x: Int?) -> Unit {
  println(x)
}

fn f1(x: T) -> Unit {
  match x {
    B(Some(_), Some(_)) => println(42)
    B(None, Some(_)) => println(43)
    B(_, None as y) => f(y)
  }
}

fn init {
  f1(T::B(None, None))
}
