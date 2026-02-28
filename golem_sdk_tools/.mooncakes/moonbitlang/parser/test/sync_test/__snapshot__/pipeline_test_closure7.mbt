// only non-rec well-known functions
fn f(x: Int, y: Int, z: Int) -> Int {
    letrec f1 = fn() { f2() + x }
    and f2 = fn() { f3() + y }
    and f3 = fn() { z }
    f1() + f2() + f3()
}

fn init {
  println(f(1,2,3))
}

// mutually-recursive well-known function
fn g(x: Int, y: Int, z: Int) -> Int {
  letrec g1 = fn (n: Int) { if n > 0 { x } else {g2(n-1)} }
  and g2 = fn (n: Int) { if n > 0 { y } else {g3(n-1)} }
  and g3 = fn (n: Int) { if n > 0 { z } else {g1(n-1)} }
  g1(3) + g2(4) + g3(5)
}

fn init {
  println(g(1,2,3))
}

fn[T] id(x: T) -> T { x }

// mutually-recursive well-known and non-well-known mixed
fn h(x: Int, y: Int, z: Int, w: Int) -> Int {
  letrec h1 = fn(n: Int) { if n > 0 { x } else {h2(n-1)} }
  and h2 = fn(n: Int) { if n > 0 { y } else {h3(n-1)} }
  and h3 = fn(n: Int) { if n > 0 { z } else {id(h4)(n-1)} }
  and h4 = fn(n: Int) { if n > 0 { w } else {h1(n-1)} }
  h1(3) + h2(4) + id(h3)(5) + h4(6)
}
fn init {
  println(h(1,2,3,4))
}

// local well-known in local well-known
fn f2(x: Int) -> Int {
  fn layer1(y: Int) {
    fn layer2() { x + y }
    ignore(layer2())
    1 + layer2()
  }
  ignore(layer1(1))
  1 + layer1(1)
}

fn init {
  println(f2(1)) // 4
}

// local well-known in local closure
fn f3(x: Int) -> Int {
  fn layer1(y: Int) {
    fn layer2() { x + y }
    ignore(layer2())
    1 + layer2()
  }
  1 + id(layer1)(1)
}

fn init {
  println(f3(1)) // 4
}

// local well-known in local recursive well-known
fn r1(x: Int) -> Int {
  fn rec(y: Int) {
    fn inner() { x + rec(y - 1) }
    if y == 0 { 0 } else { ignore(inner()); 1 + inner() }
  }
  1 + rec(x)
}

fn init {
  println(r1(2))
}

// local well-known in local recursive closure
fn r2(x: Int) -> Int {
  fn rec(y: Int) {
    fn inner() { x + id(rec)(y - 1) }
    if y == 0 { 0 } else { ignore(inner()); 1 + inner() }
  }
  1 + rec(x)
}

fn init {
  println(r2(2))
}

// local closure in local recursive well-known
fn r3(x: Int) -> Int {
  fn rec(y: Int) {
    fn inner() { x + rec(y - 1) }
    if y == 0 { 0 } else { 1 + id(inner)() }
  }
  1 + rec(x)
}

fn init {
  println(r3(2))
}
