
enum T {
  A(Int)
  B(Int?)
  C(Int, Int)
}

fn f1(x: T) -> Unit {
  match x {
    B(Some(x)) | A(x) => println(x)
    B(None as y) =>
      match y {
        Some(_) => println("bad")
        None => println("good")
      }
    C(x, 1 as y) | C(2 as y, x) => println(x - y)
    C(x, y) => println(x * y)
  }
}

fn init {
  f1(T::A(1))
  f1(T::B(None))
  f1(T::B(Some(2)))
  f1(T::C(2, 1))
  f1(T::C(3, 1))
  f1(T::C(2, 3))
  f1(T::C(1, 2))
}
