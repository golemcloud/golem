fn test_struct(x : Int) -> Unit {
  struct S {
    mut x : Int
    y : Int
  }
  let a = S::{ x, y: 2 }
  fn g(s : S) {
    let a = s.x
    s.x = s.y
    a
  }

  let _ = g // prevent inlining
  println(g(a) + a.x)
}

fn test_enum() -> Unit {
  enum E {
    C(mut x~ : Int, mut y~ : Int)
  }
  fn swap_xy(x : E) -> Unit {
    match x {
      C(y~, ..) as c => {
        c.y = c.x
        c.x = y
      }
    }
  }

  fn print_e(x : E) -> Unit {
    match x {
      C(x~, y~) => println("x: \{x}, y: \{y}")
    }
  }

  let e = C(x=1, y=2)
  print_e(e)
  swap_xy(e)
  print_e(e)
}

fn main {
  struct S {
    a : Int
    b : Int
  }
  fn f(s : S) {
    s.a + s.b
  }

  println(f({ a: 1, b: 2 }))
  println(f({ a: 3, b: 4 }))
  test_struct(40)
  test_enum()
}
