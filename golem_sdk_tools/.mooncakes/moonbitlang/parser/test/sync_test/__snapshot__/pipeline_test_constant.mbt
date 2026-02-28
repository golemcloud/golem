const C1 = 42
const C2 = "abc"
const C3 = 4.2
const C4 = '字'

fn init {
  println("C1 = \{C1}")
  println("C2 = \{C2}")
  println("C3 = \{C3}")
  println("C4 = \{C4}")
}

const ZERO = 0
const ONE = 1
const TWO = 2

fn f(x : Int) -> Unit {
  match x {
    ZERO => println("zero")
    ONE => println("one")
    TWO => println("two")
    _ => ()
  }
}

fn init {
  f(0)
  f(1)
  f(2)
  f(3)
}

const EMPTY = ""
const ANSWER = "42"

fn g(x : String) -> Unit {
  match x {
    EMPTY => ()
    ANSWER => println("answer")
    _ => println("got: \{x.escape()}")
  }
}

fn init {
  g("")
  g("42")
  g("24")
}
