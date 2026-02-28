enum E {
  C1
  C2(Int, x~ : Int, Int, y~ : Int)
}

fn f1(x : E) -> Unit {
  match x {
    C1 => println("C1")
    C2(a, b, ..) as c => {
      let x = c.x
      let y = c.y
      println("C2(\{a}, x=\{x}, \{b}, y=\{y})")
    }
  }
}

fn f2(x : E) -> Unit {
  match x {
    C1 => println("C1")
    C2(_) as c => println(c.x + c.y)
  }
}

fn init {
  f1(C2(1, x=2, 3, y=4))
  f2(C2(1, x=2, 3, y=4))
}

struct NonRec(Int)
struct Rec(Rec?)

fn f3(x : NonRec) -> Unit {
  match x {
    NonRec(_) as n => println(n.0)
  }
}

fn f4(x : Rec) -> Unit {
  match x {
    Rec(None) as r => println(r.0 is None)
    Rec(_) as r => println(r.0 is None)
  }
}

fn init {
  f3(NonRec(42))
  f4(Rec(None))
  f4(Rec(Some(Rec(None))))
}
