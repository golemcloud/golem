struct T(Int?) derive (Show)

fn[X, Y] apply(f: (X) -> Y, x: X) -> Y {
  f(x)
}

fn init {
  let a = apply(T::T(_), Option::Some(1))
  println(a)
}

fn init {
  let a = apply(Some(_), 1)
  println(a)
}

struct Point(Int,Int,Int) derive(ToJson)

fn init {
  println(Point(1,2,3).to_json().stringify())
}
