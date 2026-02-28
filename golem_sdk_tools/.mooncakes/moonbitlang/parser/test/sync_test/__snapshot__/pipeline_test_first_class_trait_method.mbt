trait Obj {
  f(Self) -> Unit
}

impl Obj for Int with f(self) {
  println(self)
}

// 1. object method passed as first class value
fn init {
  [ 42 ].each(Obj::f)
  [ 42 as &Obj ].each(Obj::f)
}

// 2. local method passed as first class value,
//    and later instantiated to object method
fn[X : Obj] poly(x : X) -> Unit {
  [ x ].each(Obj::f)
}

fn init {
  poly(42)
  poly(42 as &Obj)
}
