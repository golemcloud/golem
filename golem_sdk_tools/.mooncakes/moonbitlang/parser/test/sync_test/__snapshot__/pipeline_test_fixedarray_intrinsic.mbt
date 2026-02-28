fn[T] make(size : Int) -> UninitializedArray[T] = "%fixedarray.make_uninit"

fn main {
  let a = make(777)
  a[0] = 1
  a[1] = 2
  println(a[0]+a[1])

  let _ : UninitializedArray[Int] = make(0)  

}
