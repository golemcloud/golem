
struct T(T?) derive (Show)

fn open(x: T) -> T {
  match x {
    T(Some(x)) => x
    T(None) => T::T(None)
  }
}

fn wrap(x: T) -> T {
  T::T(Some(x))
}

fn init {
  let x1 = T::T(Some(T::T(None)))
  let x2 = wrap(x1)
  let x3 = wrap(x2)
  let x4 = wrap(x3)
  println(x4)
  let y3 = open(x4)
  let y2 = open(y3)
  let y1 = open(y2)
  println(y1)
  let y0 = open(y1)
  let yy = open(y0)
  println(yy)
}
