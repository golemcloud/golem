enum E {
  A
  B
} derive(Show)

fn f(x : E) -> Unit {
  match x {
    A as a => println(a)
    B as b => println(b)
  }
}

fn init {
  f(A)
  f(B)
}
