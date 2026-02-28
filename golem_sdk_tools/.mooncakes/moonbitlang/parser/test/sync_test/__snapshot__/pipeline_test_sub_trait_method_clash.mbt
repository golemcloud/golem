trait A {
  f(Self) -> Int
}

trait B: A {
  f(Self) -> Int
}

impl A for Int with f(i) { i }
impl B for Int with f(i) { i + 1 }

fn use_A(a : &A) -> Int {
  a.f()
}

fn main {
  let b = 0 as &B
  println(A::f(b))
  println(B::f(b))
  println(use_A(b))
}

