const C = 2


pub fn hello(x~ : Int = C + 1, y~ : Int = C + 2, z~ : Int = C + 3) -> Unit {
  println("x:\{x}\ny:\{y}\nz:\{z}\n")
}

fn main {
  hello()
}