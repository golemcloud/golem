trait Obj {
  f(Self, Int) -> Unit
}

impl Obj for Unit with f(_self, x) {
  println("Obj::f[Int](\{x})")
}

fn init {
  let obj = () as &Obj
  obj..f(1)..f(2)..f(3)..f(4)
}
