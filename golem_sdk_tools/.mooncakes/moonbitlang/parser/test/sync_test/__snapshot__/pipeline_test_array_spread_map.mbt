///|
let map : @sorted_map.SortedMap[Int, String] = @sorted_map.from_array([
  (3, "c"),
  (2, "b"),
  (1, "a"),
])

///|
fn init {
  let v = [..map, (4, "d")]
  println(v)
}
