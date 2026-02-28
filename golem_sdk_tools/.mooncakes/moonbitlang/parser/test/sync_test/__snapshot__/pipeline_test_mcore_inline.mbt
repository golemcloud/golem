///|
#locals(f)
fn[T] iter(arr : Array[T], f : (T) -> Unit) -> Unit {
  for i in 0..<arr.length() {
    f(arr[i])
  }
}

///|
fn rec_iter() -> Unit {
  let arr = [1, 2, 3, 4, 5]
  iter(arr, 
    x => {
      let subarr = Array::make(x, x)
      iter(subarr, println)
      println(x)
    }
  )
}

///|
fn main {
  rec_iter()
}
