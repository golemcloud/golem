trait HasLength  {
  length(Self) -> Int
}

fn[A : HasLength] empty(this : A) -> Bool {
  this.length() == 0
}

fn init {
  println(empty(([1]: FixedArray[_])))
  println(empty(FixedArray::make(1, 1)))
  println(empty(""))
}

impl[X] HasLength for FixedArray[X] with length(self) {
  self.length()
}

impl HasLength for String with length(self) {
  self.length()
}
