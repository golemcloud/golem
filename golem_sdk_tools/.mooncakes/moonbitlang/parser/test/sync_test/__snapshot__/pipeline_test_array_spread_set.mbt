///|
let set : @sorted_set.SortedSet[Int] = @sorted_set.from_array([3, 1, 2])

///|
fn init {
  let v = [..set, 4]
  println(v)
}
