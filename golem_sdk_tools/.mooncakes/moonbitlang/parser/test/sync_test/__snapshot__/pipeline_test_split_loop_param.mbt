fn f1(x: Int, y: Int) -> Unit {
    loop (x, y) {
        (0, 1) => println("xx")
        (a, b) => continue (a - 1, b - 1)
    }
}

fn f2(x: Int, y: Int) -> Unit {
    loop (x, y) {
        (0, 1) => println("xx")
        (a, b) => continue h(a - 1, b - 1)
    }
}

fn f3(z: (Int, Int)) -> Unit {
    loop z {
        (0, 1) => println("xx")
        (a, b) => continue (a - 1, b - 1)
    }
}

fn f4(z: (Int, Int)) -> Unit {
    loop z {
        (0, 1) => println("xx")
        w => continue h2(w)
    }
}

fn h(x: Int, y: Int) -> (Int, Int) {
  (x, y)
}

fn h2(z: (Int, Int)) -> (Int, Int) {
  (z.0 - 1, z.1 - 1)
}

struct S {
  x: Int
  y: String
}

fn g1(s: S) -> Unit {
  loop s {
    { x: 0, y } => println(y)
    { x, y } => continue { x: x - 1, y: "\{y}.\{x}" }
}
}

fn main {
  f1(2, 3)
  f2(1, 2)
  f3((3, 4))
  f4((4, 5))
  g1(S::{ x: 2, y: "hello" })
}
