fn f() -> Unit {
  enum T {
    A(Int)
    B(String)
  } derive(Eq)

  fn t(x: T, y: T) {
    x == y
  }

  println(t(A(1), A(2)))
  println(t(B("xxx"), B("xxx")))
}

fn[A: Eq, B: Eq + Show, C: Show] g(x: A, y: B, z: C) -> &Show {
  enum T {
    A(A)
    B(B)
  } derive(Eq)
  struct S {
    a: T
    b: B
  } derive(Eq)
  enum U {
    B(B)
    C(C, Int)
  } derive(Show)
  struct W {
    u: U
    v: String
  } derive(Show)

  fn t(x: T, y: T) {
    x == y
  }

  let t1 : T = A(x)
  let t2 : T = B(y)
  let s1 : S = { a: t1, b: y }
  let s2 : S = { a: t2, b: y }

  println(t(t1, t2))
  println(t(t2, t2))
  println(use_eq(t1, t1))
  println(use_eq(s1, s2))
  println(use_eq(s2, s2))
  let u1 : U = C(z, 42)
  let u2 : U = B(y)
  let w1 : W = { u: u1, v: "v1" }
  let w2 : W = { u: u2, v: "v2" }
  use_show(w1)
  use_show(w2)
  w1
}

fn[T : Eq] use_eq(x : T, y : T) -> Bool {
  x == y
}

fn[T: Show] use_show(x: T) -> Unit {
  println(x.to_string())
}

fn use_show_obj(x: &Show) -> Unit {
  println(x.to_string())
}

fn main {
  f()
  let w = g(1, "xxx", 3.14)
  use_show_obj(w)
}
