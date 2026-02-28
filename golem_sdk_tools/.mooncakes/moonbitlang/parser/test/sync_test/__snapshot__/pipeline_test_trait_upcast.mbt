struct S {
  s: Int
}

enum E {
  E(Int)
}

impl Show for S with output(self, logger) {
  logger.write_string(self.s.to_string())
}

impl Show for E with output(self, logger) {
  match self {
    E(i) => logger.write_string(i.to_string())
  }
}

fn f() -> Int {
  0
}

fn g(x: Int) -> Int {
  x
}

fn highorder(f:(Int) -> &Show, x: Int) -> Unit {
  test_show(f(x))
}

fn h(x: Int) -> Int? {
  if x == 14 {
    Some(x)
  } else {
    None
  }
}

fn test_show(x: &Show) -> Unit {
  println(x.to_string())
}

fn[T] id(x: T) -> T {
  x
}

fn[T: Add] add(x: T, y: T) -> T {
  x + y
}

fn init {
  test_show(add(1, -2))
}

fn init {
  test_show(f()) // upcast application
  test_show(1 |> g) // upcast pipe
  2 |> g |> test_show // upcast lhs of pipe
  test_show(S::{ s: 3 }) // upcast qualified struct
  test_show({ s: 4 }) // upcast unqualified struct
  test_show(S::{ s: 5 }) // upcast struct
  test_show({ s: 6 }) // upcast struct
  test_show({ s: 7 }) // upcast struct
  test_show(8) // upcast constant
  test_show((4 + 5: Int)) // upcast constraint
  let ten = 10
  test_show(ten) // upcast ident
  let s11 = { s: 11 }
  test_show(s11.s) // upcast field
  test_show(10 + 2)
  test_show(13 as &Show)
  test_show(E::E(15))
  test_show(test_if(true))
  test_show(test_if(false))
  test_show(test_match(0))
  test_show(test_match(1))
  highorder(fn(x) { x + 10 }, 10)
//  highorder(fn(x) -> Int { x + 10 }, 11)
  test_show(id(22))
}

fn test_if(b: Bool) -> &Show {
  if b { 16 } else { 17 }
}

fn test_match(x: Int) -> &Show {
  match x {
    0 => 18
    _ => 19
  }
}

fn[T:Show] test_array(x: FixedArray[T]) -> T {
  println(x[0].to_string());
  x[0]
}

fn init {
  test_show(test_array([1, 2, 3])) // no implicit upcast on each single element
  test_show(test_array([1 as &Show, 2, 3])) // this is completely different!
}
