///|
struct P {
  mut s : StringView
}

///|
fn P::read(self : P) -> Char? {
  match self.s {
    [] => None
    [c, .. cs] => {
      self.s = cs
      Some(c)
    }
  }
}

///|
fn f(s : String) -> Int {
  let p = P::{ s: s.view() }
  let mut i = 0
  while i is (0..=10) && p.read() is Some(c) && c >= 'a' && c <= 'z' {
    i = i + 1
    println(c)
  } else {
    i
  }
}

fn t() -> Unit raise {
  let a = Some(3)
  while a is Some(a) {
    println(a)
    if a > 2 {
      fail("a > 2")
    }
  }
}

///|
fn main {
  println(f("hello"))
  println(f("hellolongword"))
  println(f("Hello"))
  try {
    t()
  } catch {
    error => println(error)
  }
}
