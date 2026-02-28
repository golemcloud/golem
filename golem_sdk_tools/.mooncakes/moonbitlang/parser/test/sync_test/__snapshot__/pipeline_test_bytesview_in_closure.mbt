///|
pub fn f1(x : BytesView) -> Int {
  app(a => a + x.length(), x[0].to_int())
}

///|
pub fn f2(x : BytesView, y : BytesView) -> Int {
  app(a => a + x.length() + y.length(), x[0].to_int() + y[0].to_int())
}

///|
pub fn f3(x : BytesView) -> Int {
  fn f() -> Int {
    x.length()
  }

  f() + f()
}

///|
pub fn f4(x : BytesView, y : BytesView) -> Int {
  fn f() -> Int {
    x.length() + y.length()
  }

  f() + f()
}

///|
pub fn f5(x : BytesView, i : Int) -> Int {
  letrec fa = fn (a : Int) -> Int {
    if a > 0 {
      fb(a - 1)
    } else {
      x.length() + i
    }
  }

  and fb = fn(a : Int) -> Int {
    if a > 0 {
      fa(a - 1)
    } else {
      x[i].to_int()
    }
  }

  fa(5) + fa(6)
}

///|
fn app(f : (Int) -> Int, a : Int) -> Int {
  f(a)
}

///|
fn main {
  let x = Bytes::makei(10, i => i.to_byte())
  let y = Bytes::makei(10, i => (i + 1).to_byte())
  println(f1(x[1:9]))
  println(f2(x[1:9], y[2:8]))
  println(f3(x[1:9]))
  println(f4(x[1:9], y[2:8]))
  println(f5(x[1:9], 5))
}
