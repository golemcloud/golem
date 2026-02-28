struct F1( (Int) -> Bool)
struct F2( (Int) -> Int)
struct F3[T]( (T) -> Unit)

fn init {
  letrec even: F1 = x => if x == 0 { true } else { odd(x - 1) }
  and odd: F1 = x => if x == 0 { false } else { even(x - 1) }

  println(even(10))
}

fn init {
  letrec fib: F2 = x => if x == 1 || x == 2 { 1 } else { fib(x - 1) + fib(x - 2) }

  println(fib(10))
}

fn init {
  letrec f1: F3[Int] = fn(x) { f2(x) }
  and f2 = fn(x) { f3(x) }
  and f3 = fn(x) { println(x) }

  f1(10)
}

struct R[X]((R[X]) -> X)

fn[X, Y] y(f : ((X) -> Y) -> (X) -> Y) -> (X) -> Y {
  letrec ff: R[(X) -> Y] = fn(x: R[(X) -> Y]) -> (X) -> Y {
    fn(a) { f((x.0)(x))(a) }
  }
  ff(R::R(fn(x) { fn (a) { f((x.0)(x))(a) } }))
}

fn factx(f: ((Int) -> Int)) -> (Int) -> Int {
  fn(n: Int) -> Int {
    if n <= 1 { 1 } else { n * f(n-1)}
  }
}

fn init {
  
  let fact = y(factx)
  println(fact(10))
}
