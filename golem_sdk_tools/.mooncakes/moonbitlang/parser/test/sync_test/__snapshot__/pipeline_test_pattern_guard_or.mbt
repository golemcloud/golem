///|
enum E {
  A(Int)
  B(Int)
  C
} derive(Show)

///|
fn f1(x : E, y : E) -> Unit {
  match (x, y) {
    (C, C) => println("both c")
    (A(x), b) | (b, A(x)) if b is B(i) && i > x => {
      println("guard!")
      println(b)
      println(x)
      println(i)
    }
    _ => println("else")
  }
}

fn f2(x : E, y : E) -> Unit {
  match (x, y) {
    (C, C) => println("both c")
    (A(x), b) | (b, A(x)) if b is (B(i) | A(i)) && i > x => {
      println("guard!")
      println(b)
      println(x)
      println(i)
    }
    _ => println("else")
  }
}

///|
fn main {
  println("test f1")
  f1(C, C)
  f1(A(1), B(2))
  f1(B(1), A(2))
  println("test f2")
  f2(C, C)
  f2(A(1), B(2))
  f2(B(1), A(2))
  f2(A(1), A(2))
  f2(A(2), A(1))
}
