fn init {
  let arr: FixedArray[_] = [17.13, 20.23, 8.29]
  println(arr[0])
  println(arr[1])
  arr[2] = 9.29
  println(arr[2])
  arr[1] = 3.14
  println(arr[1])
  println(arr.length())
}

struct T {
  a: Double
  mut b: Double
  mut c: Int
}

fn T::print_t(self: T) -> Unit {
  println("\na: ")
  println(self.a)
  println("b: ")
  println(self.b)
  println("c: ")
  println(self.c)

}

fn f(x: T, b: Double, c: Int) -> Unit {
  x.print_t()
  x.b = b
  x.c = c
  x.print_t()
}

fn init {
  let x = { a: 1.1, b: 2.2, c: 3 }
  f(x, 22.22, 33)
}

fn f1(arr: FixedArray[Double]) -> Unit {
  match arr {
    [] => println("nothing\n")
    [x] => println(x)
    [_, y] => println(y)
    [_, _, z] => println(z)
    [.. _, v, u, w] => { println(v); println(u); println(w) }
  }
}

fn init {
  f1([])
  f1([1.1 ])
  f1([2.1, 2.2])
  f1([3.1, 3.2, 3.3])
  f1([4.1, 4.2, 4.3, 4.4])
  f1([5.1, 5.2, 5.3, 5.4, 5.5 ])
}


fn float_down_to_0(x: Double, f: (Double) -> Unit) -> Unit {
  if x >= 0.0 {
    f(x)
    float_down_to_0(x - 1.0, f)
  }
}

fn init {
  float_down_to_0(10.5, fn(x) {
    println(x)

  })
}

enum H[X, Y] {
  A
  D
  E (X)
  F (Y)
}

let h:((Double, H[Double, Double])) -> Double = fn(a) {
  match a {
    (x, D) => x * 3.0
    (x, E(y)) => x + y
    (x, F(y)) => x - y
    (y, A) => y / 1.1
  }
}

let h1:((Double, H[Double, Double])) -> Double = fn(a) {
  (match a {
    (x, D) => x * 3.0
    (x, E(y)) => x + y
    (x, F(y)) => x - y
    (y, A) => y / 1.1
  }) * 3.0
}

fn h2(x: Double) -> Double {
  match x {
    1.0 => 1.5
    2.0 => 2.5
    _ => x + 1.5
  }
}

fn init {
  println(h((1.4, H::D)))
  println(h((0.2, H::E(5.0))))
  println(h((7.3, H::F(1.1))))
  println(h((1.21, H::A)))
}

fn init {
  println(h1((1.4, H::D)))
  println(h1((0.2, H::E(5.0))))
  println(h1((7.3, H::F(1.1))))
  println(h1((1.21, H::A)))
}

fn init {
  println(h2(1.0))
  println(h2(2.0))
  println(h2(3.0))
  println(if (3.0 > 4.0) { h2(4.0) } else { h2(5.0) })
}
