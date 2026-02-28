struct MyInt(Int)

impl Eq for MyInt with equal(x, y) = "%i32_eq"

fn init {
  println(MyInt(4) == MyInt(2))
}
