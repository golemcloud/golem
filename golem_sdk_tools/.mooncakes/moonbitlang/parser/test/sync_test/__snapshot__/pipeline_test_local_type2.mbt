fn[T : Show] test_generic(x : T) -> Unit {
  struct D {
    a : T
    b : Array[T]
    c : String
  }
  fn to_string(x : D) -> String {
    x.a.to_string() + x.b.to_string() + x.c
  }

  let d1 = D::{ a: x, b: [x, x], c: "1" }
  println(to_string(d1))
  let d2 = D::{ ..d1, c: "2" }
  println(to_string(d2))
}

fn[T : Show] test_generic2(x : T) -> Unit {
  enum E {
    A(T)
    B(Array[T])
    C(String)
  }
  fn to_string(x : E) -> String {
    match x {
      A(a) => a.to_string()
      B(b) => b.to_string()
      C(c) => c
    }
  }

  println(to_string(A(x)))
  println(to_string(B([x, x])))
  println(to_string(C("1")))
}

fn[T : Show] test_generic3(x : T) -> Unit {
  enum E {
    A(Int)
    B(String)
  }
  fn f(e : E) -> Unit {
    match e {
      A(a) => println(a)
      B(b) => println(b)
    }
  }

  f(A(42))
  f(B(x.to_string()))
}

fn main {
  test_generic(42)
  test_generic("hello")
  test_generic2(42)
  test_generic2("hello")
  test_generic3(42)
  test_generic3("hello")
}
