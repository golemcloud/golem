struct MyInt(Int)

fn MyInt::equal(x : MyInt, y : MyInt) -> Bool {
  x.0 == y.0
}

impl Eq for MyInt with equal(x, y) {
  x.0 == 42 || y.0 == 42 || x.0 == y.0
}

fn init {
  println(MyInt(42) == MyInt(24))
}
