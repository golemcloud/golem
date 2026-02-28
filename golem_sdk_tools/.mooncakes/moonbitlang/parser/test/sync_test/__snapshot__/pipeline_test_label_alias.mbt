///|
#label_migration(x, alias=xx)
#label_migration(x, alias=xxx)
#label_migration(y, alias=yy)
#label_migration(z, alias=zz)
fn f(x~ : Int, y? : Int = 42, z? : Int) -> Unit {
  println("x:\{x}, y:\{y}, z:\{z}")
}

///|
fn main {
  f(x=1, y=2, z=3)
  f(xx=1, yy=2, zz=3)
  f(xx=1, yy=2)
  f(xx=1, zz=3)
  f(xxx=1, yy=2, zz=3)
  f(xxx=1, yy=2)
  f(xxx=1, zz=3)
  f(x=1, yy=2, zz=3)
  f(x=1, yy=2)
  f(x=1, zz=3)
}
