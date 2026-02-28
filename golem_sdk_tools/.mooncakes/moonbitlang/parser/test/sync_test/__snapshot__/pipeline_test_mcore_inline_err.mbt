///|
#locals(f)
fn[T, U] map_err(arr : ArrayView[T], f : (T) -> U raise Err) -> ArrayView[U] raise Err {
  let result = []
  for a in arr {
    result.push(f(a))
  }
  if result.length() > 8 {
    raise E1
  }
  result[:]
}

///|
suberror Err {
  E1
  E2
} derive(Show)

///|
fn main {
  let arr = [1, 2, 3, 4, 5]
  println(try? map_err(arr[:], x =>
      if x < 0 {
        raise E2
      } else {
        if x % 2 == 0 {
          return x + 2
        }
        x + 1
      }
  ))
  println(try? map_err([..arr, ..arr][:], x =>
      if x < 0 {
        raise E2
      } else {
        if x % 2 == 0 {
          return x + 2
        }
        x + 1
      }
  ))
  let arr1 = [-1, 2, 3, 4, 5]
  println(try? map_err(arr1[:], x =>
      if x < 0 {
        raise E2
      } else {
        if x % 2 == 0 {
          return x + 2
        }
        x + 1
      }
  ))
}
