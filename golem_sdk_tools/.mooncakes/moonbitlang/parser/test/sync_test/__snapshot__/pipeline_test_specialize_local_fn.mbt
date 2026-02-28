///|
#locals(value)
fn[T] makei(length : Int, value : (Int) -> T) -> Array[T] {
  if length <= 0 {
    []
  } else {
    let array = Array::make(length, value(0))
    for i in 1..<length {
      array[i] = value(i)
    }
    array
  }
}

///|
#locals(f)
fn[T] find(arr : Array[T], f : (T) -> Bool) -> Int? {
  for i, a in arr {
    if f(a) {
      return Some(i)
    }
  }
  None
}

///|
fn[T : Show] f(x : T) -> Bool {
  x.to_string().length() == 6
}

///|
fn[T] g(x : T) -> T {
  x
}

///|
fn h(x : Int) -> String {
  x.to_string()
}

///|
fn main {
  let arr = makei(100001, g)
  println(find(arr, f))
  let arr1 = makei(100001, h)
  println(find(arr1, f))
}
