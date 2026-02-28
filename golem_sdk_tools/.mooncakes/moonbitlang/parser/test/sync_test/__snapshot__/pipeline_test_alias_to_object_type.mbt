trait I {
  f(Self) -> Unit
}

impl I for Int with f(self) {
  println(self)
}

type Obj = &I

fn init {
  let obj = 42 as Obj
  obj.f()
}
