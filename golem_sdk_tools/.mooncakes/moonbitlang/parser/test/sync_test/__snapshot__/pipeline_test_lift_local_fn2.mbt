pub fn f(x: Int) -> Int {
  letrec g = fn() { x }
  and h = fn() {
    x + g() + g()
  }
  h() + h()
}

fn main {
  println(f(42))
}
