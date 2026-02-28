///|
fn main {
  letrec fib = n => if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }

  println(fib(8)) // Should print 55
  letrec nonrec = (a, b) => a + b

  println(nonrec(5, 10)) // Should print 15
  letrec forever = () => (forever() : Unit)

  ignore(forever)
}


