#alias(A)
trait I {
  f(Self) -> Unit = _
}

impl A with f(self) {
  println("I::f default impl")
}

impl A for Int with f(self) {
  println("I::f for Int")
}

impl A for String

fn &A::g(self : &A) -> Unit {
  self.f()
}

fn[X : A] use_A(x : X) -> Unit {
  x.f()
}

trait Derived : A {
  g(Self) -> Unit
}

impl Derived for Int with g(self) {
  A::f(self)
}

fn init {
  A::f(42)
  use_A("abc")
  let obj = 42 as &A
  &A::g(obj)
  Derived::g(42)
}
