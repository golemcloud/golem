///|
suberror LE {
  LE(Int)
} derive(Show)

///| basic test
fn main {
  struct LT(Int) derive(Show)
  fn f1(x : Int) -> LT raise LE {
    if x > 0 {
      LT(x)
    } else {
      raise LE(x)
    }
  }
  println(try? f1(42))
  println(try? f1(-1))
}

// test recursive newtype
fn init {
  struct T((S) -> Int)
  struct S(T)
  letrec f = fn(t: T) -> Int {
    (t.0)(S(t))
  }
  and use_t = fn(t: T) -> Int {
    f(t)
  }
  and use_s = fn(s: S) -> Int {
    f(s.0)
  }
  let t : T = T(fn (_x) { 42 })
  println(use_t(t))
  let t = T::T(fn (_x) { 43 })
  println(use_t(t))
  let s = S(t)
  println(use_s(s))
}

fn[A: Show, B: Show] f(a: A, b: B) -> Unit {
  struct A(B, Int) derive(Show)
  struct B {
    f1: Array[C]
    f2: String
  } derive(Show)
  enum C {
    C1
    C2(A)
  } derive(Show)
  let c1 = C1
  let b1 = { f1: [c1], f2: "b1"}
  let a1 = (b1, 43)
  let c2 = C2(A(b1, 43))
  let b2 = { f1: [c1, c2], f2: "b2"}
  let a2 = (b2, 44)
  
  println(a)
  println(b)
  println(a1)
  println(a2)
  println(b1)
  println(b2)
  println(c1)
  println(c2)
}

///|
fn init {
  f("xxx", 42)
}
