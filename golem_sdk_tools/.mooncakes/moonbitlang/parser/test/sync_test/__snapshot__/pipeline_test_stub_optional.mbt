//!build: -w -27

fn my_add(x~ : Int, y~ : Int = x) -> Int = "%i32_add"

fn init {
  println(my_add(x=1, y=2))
  println(my_add(x=2))
}
