enum T {
  A
  B (Int, Int)
  C
}

fn init {
  match (false, T::B(2, 4)) {
    (_, A) => println('0')
    (true, B(i, j)) => ()
    (_, B(i, j)) => { println(i); println(j) }
    (_, C) => println('2')
  }
}

enum H[M, N] {
  A1
  D
  E (M)
  F (N)
}

fn init {
  let x = H::E(1)
  let i = match x {
    A1 => 0
    E(x) => x
    _ => 2
  }
  println(i)
}

fn init {
  let x = (1, 2)
  match x {
    (x, y) => println(x + y)
  }
}

