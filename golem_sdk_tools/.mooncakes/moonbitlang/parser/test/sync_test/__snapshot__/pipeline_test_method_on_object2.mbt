trait Obj {
  f(Self) -> Unit
}

impl Obj for Int with f(self) { println(self) }

// ok
fn &Obj::g(self : &Obj) -> Unit {
  println("=== g ===")
  self.f()
  self.f()
}

// ok
fn &Obj::default() -> &Obj {
  42
}

fn init {
  println("==== dot syntax ====")
  let obj = &Obj::default()
  obj..f()..g()
  println("==== qualified call ====")
  &Obj::f(obj)
  &Obj::g(obj)
}

trait I {
  f(Self) -> Int
}

// ok
impl I for &Obj with f(self) {
  println("=== I::f ===")
  self.f()
  43
}

fn init {
  println("==== implement other traits ====")
  Obj::f(42)
  I::f(&Obj::default()) |> println
}
