fn sub(lhs~ : Int, rhs~ : Int) -> Int = "%i32_sub"

fn init {
  println(sub(lhs=2, rhs=1))
  println(sub(rhs=2, lhs=1))
}
