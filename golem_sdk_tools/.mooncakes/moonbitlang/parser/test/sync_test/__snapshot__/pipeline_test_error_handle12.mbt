fn[T] f(x : T) -> T {
  x
}

fn g() -> Int raise {
  fail("xxx")
}

fn h() -> Int {
  42
}

fn main {
  let _ = f(g)
  let _ = f(h)

}
