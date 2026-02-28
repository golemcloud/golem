struct Box[T] {
  value : T
}

fn[T, U] Box::map(self : Box[T], f : (T) -> U) -> Box[U] {
  Box::{ value: f(self.value) }
}

trait Describe {
  describe(Self) -> String
}

impl[T : Show] Describe for Box[T] with describe(self) {
  "Box(" + self.value.to_string() + ")"
}

fn init {
  let b = Box::{ value: 3 }
  let b2 = b.map(x => x + 1)
  let _ = Describe::describe(b2)
  ()
}
