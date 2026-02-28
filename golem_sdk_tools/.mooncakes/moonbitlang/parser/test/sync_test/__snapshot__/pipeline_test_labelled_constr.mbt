enum E {
  C(Int, x~ : Int, Int, y~ : Int)
}

let c1: E = C(1, x=2, 3, y=4)
let c2: E = C(y=4, x=2, 1, 3)
let c3: E = C(y=4, 1, x=2, 3)
let c4: E = C(1, x=2, 42, y=4)
let c5: E = C(1, x=42, 3, y=4)

fn f1(x : E) -> Unit {
  match x {
    C(a, x~, b, y~) => println("\{a}, \{x}, \{b}, \{y}")
  }
}

fn f2(x : E) -> Unit {
  match x {
    C(y~, x~, a, b) => println("\{a}, \{x}, \{b}, \{y}")
  }
}

fn f3(x : E) -> Unit {
  match x {
    C(y~, x=(42 as x), a, b) => println("\{a}, \{x}, \{b}, \{y}")
    C(y~, a, 42 as b, x~) => println("\{a}, \{x}, \{b}, \{y}")
    _ => println("otherwise")
  }
}

fn init {
  println("===")
  f1(c1); f2(c1); f3(c1)
  println("===")
  f1(c2); f2(c2); f3(c2)
  println("===")
  f1(c3); f2(c3); f3(c3)
  println("===")
  f1(c4); f2(c4); f3(c4)
  println("===")
  f1(c5); f2(c5); f3(c5)
}
