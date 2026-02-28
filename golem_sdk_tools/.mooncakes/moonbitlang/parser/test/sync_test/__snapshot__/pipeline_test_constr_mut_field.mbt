enum E {
  C(mut x~ : Int, mut y~ : Int)
} derive(Show)

fn swap_xy(x : E) -> Unit {
  match x {
    C(y~, ..) as c => {
      c.y = c.x
      c.x = y
    }
  }
}

fn init {
  let e : E = C(x=1, y=2)
  println(e)
  swap_xy(e)
  println(e)
}
