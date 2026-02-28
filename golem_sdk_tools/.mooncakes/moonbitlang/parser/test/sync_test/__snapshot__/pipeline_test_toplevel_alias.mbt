fn[T : Show] f(x : T) -> String {
  x.to_string()
}

let a : (Int) -> String = f

let b : (Int) -> String = a

fn main {
  println(b(42))
}
