///|
fn[T : Show] f(x : ReadOnlyArray[T]) -> Unit {
  match x {
    [a, .. rest, b] => {
      println(a)
      println(b)
      println(rest)
    }
    [a] => println(a)
    [] => println("empty")
  }
}

///|
fn f1(x : ReadOnlyArray[(Int, Int)]) -> Unit {
  if x is [(a, b)] {
    println(a)
    println(b)
  } else {
    println(x)
  }
}

///|
let a : ReadOnlyArray[Int] = [1, 2, 3]

///|
fn main {
  f([(1, 2), (3, 4), (5, 6)])
  f([Some(78)])
  f(([] : ReadOnlyArray[Unit]))
  f1([(9, 10)])
  f1([])
  let b = [..a, 4, 5]
  println(b)
}
