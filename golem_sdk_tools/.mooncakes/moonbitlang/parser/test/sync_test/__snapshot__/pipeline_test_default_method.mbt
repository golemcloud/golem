
trait I {
  f(Self) -> Unit
  g(Self) -> Unit = _
}

impl I with g(self) {
  println("I::g.default(): calling ")
  self.f()
}

fn[X: I] useI(x: X) -> Unit {
  x.g()
}

struct T1(Int)
let t1: T1 = T1(42)

impl I for T1 with f(_self: T1) -> Unit {
  println("T1::f()")
}

fn init {
  println("===")
  I::g(t1)
  useI(t1)
  (t1 as &I).g()
}


struct T2(Int)
let t2: T2 = T2(42)

impl I for T2 with f(_self: T2) -> Unit {
  println("T2::f()")
}

impl I for T2 with g(_self) {
  println("T2::g()")
}

fn init {
  println("===")
  I::g(t2)
  useI(t2)
  (t2 as &I).g()
}


trait Sub: I {
  h(Self) -> Unit = _
}

impl Sub with h(self) {
  println("Sub::h(): calling ")
  self.g()
}

fn[X: Sub] use_Sub(x: X) -> Unit {
  x.h()
}

struct T3(Int)
let t3: T3 = T3(42)

impl I for T3 with f(_self: T3) -> Unit {}


impl Sub for T1
impl Sub for T2
impl Sub for T3 with h(_self) {
  println("I3::h()")
}

fn init {
  println("===")
  Sub::h(t1)
  use_Sub(t1)
  (t1 as &Sub).h()

  println("===")
  Sub::h(t2)
  use_Sub(t2)
  (t2 as &Sub).h()

  println("===")
  Sub::h(t3)
  use_Sub(t3)
  (t3 as &Sub).h()
}

struct T4(Int)
let t4: T4 = T4(42)

impl I for T4 with f(_self: T4) -> Unit { println("T4::f()") }
fn T4::g(_self: T4) -> Unit { println("T4::g()") }

fn init {
  println("===")
  // default method has higher precedence then method
  I::g(t4)
  useI(t4)
  t4.g()
}
