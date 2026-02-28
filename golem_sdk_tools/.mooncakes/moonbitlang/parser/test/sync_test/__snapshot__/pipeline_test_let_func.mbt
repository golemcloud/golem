fn[A, B] landins_kont(f : ((A) -> B) -> (A) -> B) -> (A) -> B {
  let mut r : (A) -> B = fn(x) { abort("") }
  let fixedpoint = f(fn(x) { r(x) })
  r = fixedpoint
  fixedpoint
}

let factorial : (Int) -> Int = {
  let g = fn(f) {
    fn(x) {
      if x == 0 {
        1
      } else {
        x * f(x - 1)
      }
    }
  }
  landins_kont(g)
}

fn init {
  println(factorial(5))
}

let with_effect: (Int) -> Int = {
  println("Halloween!")
  let a = factorial(5)
  fn(x) { x + a }
}

fn init {
  println(with_effect(10))
  println(with_effect(31))
}
