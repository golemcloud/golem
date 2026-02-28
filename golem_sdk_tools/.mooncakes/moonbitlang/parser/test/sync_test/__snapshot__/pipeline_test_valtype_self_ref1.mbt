///|
#valtype
struct S {
  a : S?
  b : (S) -> Int
  c : Array[S]
  d : (S) -> S
}

///|
fn f(s : S) -> Int {
  if s.a is Some(_) {
    s.c.length() + 1
  } else {
    s.c.length()
  }
}

///|
fn g(s : S) -> S {
  { ..s, a: None }
}

///|
fn S::make() -> S {
  { a: None, b: f, c: [], d: g }
}

///|
fn S::make1(s : S) -> S {
  { a: Some(s), b: f, c: [s], d: g }
}

///|
fn main {
  let s0 = S::make()
  let s1 = S::make1(s0)
  println((s1.b)(s0))
  println((s1.b)(s1))
  println((s1.b)((s1.d)(s1)))
}
