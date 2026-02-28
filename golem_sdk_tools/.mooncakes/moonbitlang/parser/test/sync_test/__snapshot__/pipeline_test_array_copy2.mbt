/// @intrinsic %fixedarray.copy
fn[A] array_copy(dst: UninitializedArray[A], dst_index: Int, src: FixedArray[A], src_index: Int, len: Int) -> Unit {
    for i = 0; i < len; i = i + 1 {
      dst[dst_index + i] = src[src_index + i]
    }
}

fn[X: Eq] array_eq(arr1: UninitializedArray[X], arr2: FixedArray[X], low: Int, high: Int) -> Unit {
    for i = low, j = 0; i < high; i = i + 1, j = j + 1 {
        if arr1[i] != arr2[j] {
        abort("error")
        }
    }
    println(".")
}

fn init {
  let x: FixedArray[_] = [(1, 2), (2, 3), (3, 4)]
  let y = UninitializedArray::make(6)
  array_copy(y, 1, x, 0, 3)
  array_eq(y, x, 1, 4)
}
