/// @intrinsic %array.length
fn[X] my_length(arr : Array[X]) -> Int {
  arr.length()
}

/// @intrinsic %array.unsafe_get
fn[X] my_unsafe_get(arr : Array[X], index : Int) -> X {
  arr[index]
}

/// @intrinsic %array.get
fn[X] my_get(arr : Array[X], index : Int) -> X {
  if index < 0 || index >= my_length(arr) {
    panic()
  }
  my_unsafe_get(arr, index)
}

/// @intrinsic %array.unsafe_set
fn[X] my_unsafe_set(arr : Array[X], index : Int, value : X) -> Unit {
  arr[index] = value
}

/// @intrinsic %array.set
fn[X] my_set(arr : Array[X], index : Int, value : X) -> Unit {
  if index < 0 || index >= my_length(arr) {
    panic()
  }
  my_unsafe_set(arr, index, value)
}

fn init {
  let arr = [ 1, 2, 3 ]
  println(my_length(arr))
  println(my_unsafe_get(arr, 0))
  println(my_get(arr, 1))
  my_unsafe_set(arr, 1, 3)
  my_set(arr, 2, 4)
  println(my_get(arr, 1))
  println(my_get(arr, 2))
}
