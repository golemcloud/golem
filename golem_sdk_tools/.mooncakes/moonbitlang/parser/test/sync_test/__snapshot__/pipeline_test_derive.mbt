
struct T {
  x: Int
  y: Int
} derive(Eq, Show, Compare, Default)

struct P[X, Y] {
  a: (X, Y?, Int)
  b: T
} derive(Eq, Show, Compare, Default)

enum E[X] {
  A
  B(X)
  C(X, T)
} derive (Eq, Show, Compare, Default)
struct Stock {
  tick : String
} derive(Show)

struct MyStock(String) derive(Show)

fn init {
  println(Stock::{ tick: "MSFT" })
  println(MyStock::MyStock("MSFT"))
  println("MSFT")
}

fn init {
  println({ x: 1, y: 1 } == { x: 1, y: 1 })
  println({ x: 1, y: 1 } == { x: 1, y: 2 })
  println({ x: 1, y: 1 } == { x: 2, y: 1 })

  println("=====")
  println({ a: (true, None, 3), b: { x: 3, y: 4 } } == { a: (true, None, 3), b: { x: 3, y: 4 } })
  println({ a: (true, None, 3), b: { x: 3, y: 4 } } == { a: (false, None, 3), b: { x: 3, y: 4 } })
  println({ a: (true, None, 3), b: { x: 3, y: 4 } } == { a: (true, Some('a'), 3), b: { x: 3, y: 4 } })
  println({ a: (true, None, 3), b: { x: 3, y: 4 } } == { a: (true, None, 4), b: { x: 3, y: 4 } })
  println({ a: (true, None, 3), b: { x: 3, y: 4 } } == { a: (true, None, 3), b: { x: 4, y: 4 } })

  println("=====")
  println(E::A == E::A)
  println(E::B('x') == E::B('x'))
  println(E::C('x', { x: 1, y : 2 }) == E::C('x', { x: 1, y : 2 }))
  println(E::A == E::B('x'))
  println(E::A == E::C('x', { x: 1, y : 2 }))
  println(E::B('x') == E::C('x', { x: 1, y : 2 }))
  println(E::B('x') == E::B('y'))
  println(E::C('x', { x: 1, y : 2 }) == E::C('y', { x: 1, y : 2 }))
  println(E::C('x', { x: 1, y : 2 }) == E::C('x', { x: 2, y : 2 }))
}

fn init {
  println("=====")
  println({ x: 1, y: 2 })
  println({ a: (true, Some('a'), 3), b: { x: 2, y: 3 } })
  println(E::A)
  println(E::B("345"))
  println(E::C({ x: 1, y: 2 }, { x: 3, y: 4 }))
}

fn init {
  println("=====")
  println({ x: 1, y: 2 }.compare({ x: 0, y: 2 }))
  println({ x: 1, y: 2 }.compare({ x: 1, y: 2 }))
  println({ x: 1, y: 2 }.compare({ x: 2, y: 2 }))
  println({ x: 1, y: 2 }.compare({ x: 1, y: 1 }))
  println({ x: 1, y: 2 }.compare({ x: 1, y: 2 }))
  println({ x: 1, y: 2 }.compare({ x: 1, y: 3 }))
  println({ x: 2, y: 1 }.compare({ x: 1, y: 2 }))

  println("=====")
  println((E::A : E[Int]).compare(E::A))
  println(E::A.compare(E::B(1)))
  println(E::A.compare(E::C(1, { x: 0, y: 0 })))
  println("---")
  println(E::B(1).compare(E::A))
  println(E::B(1).compare(E::B(0)))
  println(E::B(1).compare(E::B(1)))
  println(E::B(1).compare(E::B(2)))
  println(E::B(1).compare(E::C(1, { x: 0, y: 0 })))
  println("---")
  let c: E[_] = C(1, { x: 1, y: 1 })
  println(c.compare(E::A))
  println(c.compare(E::B(2)))
  println(c.compare(E::C(0, { x: 2, y: 2 })))
  println(c.compare(E::C(2, { x: 0, y: 0 })))
  println(c.compare(E::C(1, { x: 0, y: 2 })))
  println(c.compare(E::C(1, { x: 1, y: 1 })))
  println(c.compare(E::C(1, { x: 1, y: 2 })))
}

fn init {
  println("=====")
  println(T::default())
  println((P::default() : P[Bool, Int]))
  println((E::default() : E[Int]))
}
