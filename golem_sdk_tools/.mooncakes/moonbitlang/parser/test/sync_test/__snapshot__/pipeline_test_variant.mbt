enum T {
  A(Double, Double)
  B(Double)
  C
}

fn f(t: T) -> Unit {
  match t {
    A(x,y) => println(x+y)
    B(x) => println(x)
    C => println(9.28)
  }
}

fn init {
  let a = T::A(1.1, 2.2)
  let b = T::B(3.3)
  let c = T::C
  f(a)
  f(b)
  f(c)
}
