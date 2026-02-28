enum Opt[T] {
  None
  Some (T)
}

fn print() -> Unit {

}

fn[T, B] Opt::map(self: Opt[T], f: (T) -> B) -> Opt[B] {
  match self {
    Some(x) => Opt::Some(f(x))
    _ => Opt::None
  }
}

fn[T, B] my_op_mul(lhs: Opt[T], rhs: Opt[B]) -> Opt[(T, B)] {
  match lhs {
    Some(t) => {
      match rhs {
        Some(b) => Opt::Some((t, b))
        _ => Opt::None
      }
    }
    _ => Opt::None
  }
}

fn init {
  // Double,Int,Bool,String,Char
  let a = 10
  let b = false
  let c = "hello"

  // Identifier
  let d = 5 + a
  println(d)
  print()

  // Construct Opt::None
  let e: Opt[Unit] = Opt::None
  match e {
    Some(_) => println("error")
    None => println("ok")
  }

  print()

  // Construct Opt::Some(T)
  let e2 = Opt::Some(2)
  match e2 {
    Some(x) => println(x)
    _ => println("error")
  }
  print()
  let e3 = e2.map(fn(x) {(x, "abc")})
  let e4 = e3.map(fn(x) {
    let (a, b) = x
    println(b)
  })
  let e5 = my_op_mul(e2,  e3)
  let e6 = e5.map(fn(x) {
    let (a, (b, c)) = x
    println(c)
  })
  //match e5 {
  //  Some(x) => {
  //    let (a, (b, c)) = x
  //    println(a)
  //  }
  //  _ => {"None".print()}
  //}
  print()

}
